mod routes;

use axum::{
    extract::State,
    http::{HeaderName, Method},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use lumiere_auth::middleware::AuthState;
use lumiere_db::Database;
use lumiere_models::config::AppConfig;
use lumiere_nats::NatsService;
use serde_json::json;
use std::sync::Arc;
use tower_http::{
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

pub struct AppState {
    pub config: AppConfig,
    pub db: Database,
    pub redis: redis::aio::ConnectionManager,
    pub nats: NatsService,
    pub snowflake: lumiere_models::snowflake::SnowflakeGenerator,
}

impl AuthState for AppState {
    fn jwt_secret(&self) -> &str {
        &self.config.auth.jwt_secret
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Load configuration
    let config = AppConfig::load()?;

    // 2. Initialize tracing
    init_tracing();

    tracing::info!("Starting Lumiere server...");

    // 2b. Initialize Prometheus metrics
    let prometheus_handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .install_recorder()
        .expect("Failed to install Prometheus recorder");

    // 3. Connect to all services
    let db = Database::connect(&config).await?;
    db.run_pg_migrations().await?;
    db.run_scylla_migrations(&config).await?;

    let redis_client = redis::Client::open(config.redis.url.as_str())?;
    let redis = redis::aio::ConnectionManager::new(redis_client).await?;
    tracing::info!("Connected to Redis");

    let nats = NatsService::connect(&config.nats).await?;
    nats.setup_streams().await?;

    let machine_id: u16 = if std::env::var("LUMIERE_ENV").unwrap_or_default() == "production" {
        std::env::var("MACHINE_ID")
            .expect("MACHINE_ID must be set in production")
            .parse()
            .expect("MACHINE_ID must be a valid u16")
    } else {
        std::env::var("MACHINE_ID")
            .unwrap_or_else(|_| "1".into())
            .parse()
            .unwrap_or(1)
    };
    let snowflake = lumiere_models::snowflake::SnowflakeGenerator::new(machine_id);

    // Validate JWT secret in production
    if std::env::var("LUMIERE_ENV").unwrap_or_default() == "production" {
        if config.auth.jwt_secret == "lumiere_dev_jwt_secret_change_in_production" {
            panic!("JWT secret must be changed in production! Set LUMIERE__AUTH__JWT_SECRET");
        }
    }

    // 4. Build AppState
    let state = Arc::new(AppState {
        config: config.clone(),
        db,
        redis,
        nats,
        snowflake,
    });

    // 5. Build router
    let app = build_router(state.clone(), prometheus_handle);

    // 6. Start server
    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("Lumiere server listening on {}", addr);

    axum::serve(listener, app).await?;

    Ok(())
}

fn build_router(state: Arc<AppState>, prometheus_handle: metrics_exporter_prometheus::PrometheusHandle) -> Router {
    let allowed_methods = [
        Method::GET,
        Method::POST,
        Method::PATCH,
        Method::PUT,
        Method::DELETE,
    ];
    let allowed_headers = [
        axum::http::header::AUTHORIZATION,
        axum::http::header::CONTENT_TYPE,
        axum::http::header::ACCEPT,
        HeaderName::from_static("x-audit-log-reason"),
    ];
    let exposed_headers = [
        HeaderName::from_static("x-ratelimit-limit"),
        HeaderName::from_static("x-ratelimit-remaining"),
        HeaderName::from_static("x-ratelimit-reset"),
        HeaderName::from_static("retry-after"),
    ];

    let cors = if std::env::var("LUMIERE_ENV").unwrap_or_default() == "production" {
        CorsLayer::new()
            .allow_origin("https://app.lumiere.chat".parse::<axum::http::HeaderValue>().unwrap())
            .allow_methods(allowed_methods)
            .allow_headers(allowed_headers)
            .expose_headers(exposed_headers)
            .allow_credentials(true)
    } else {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(allowed_methods)
            .allow_headers(allowed_headers)
            .expose_headers(exposed_headers)
    };

    // Gateway state
    let gateway_state = Arc::new(lumiere_gateway::handler::GatewayState {
        session_manager: lumiere_gateway::session::SessionManager::new(),
        nats: state.nats.clone(),
        redis: state.redis.clone(),
        jwt_secret: state.config.auth.jwt_secret.clone(),
        db: state.db.clone(),
    });

    let metrics_handle = prometheus_handle;
    Router::new()
        .route("/health", get(health))
        .route("/health/ready", get(health_ready))
        .route("/metrics", get(move || {
            let handle = metrics_handle.clone();
            async move { handle.render() }
        }))
        .route("/gateway", get(lumiere_gateway::handler::ws_upgrade).with_state(gateway_state))
        .nest("/api/v1/auth", routes::auth::router())
        .nest("/api/v1/users", routes::users::router())
        .nest("/api/v1/servers", routes::servers::router())
        .nest("/api/v1/invites", routes::servers::invite_router())
        .nest("/api/v1/channels", routes::servers::channel_invite_router())
        .nest("/api/v1/channels", routes::channels::router())
        .nest("/api/v1/channels", routes::roles::channel_permissions_router())
        .nest("/api/v1/channels", routes::messages::router())
        .nest("/api/v1/channels", routes::reactions::router())
        .nest("/api/v1/channels", routes::typing::router())
        .nest("/api/v1/users", routes::typing::user_unread_router())
        .nest("/api/v1/servers", routes::typing::server_ack_router())
        .nest("/api/v1/servers", routes::moderation::router())
        .nest("/api/v1", routes::moderation::report_router())
        .nest("/api/v1/channels", routes::webhooks::router())
        .nest("/api/v1/webhooks", routes::webhooks::webhook_exec_router())
        .nest("/api/v1/applications", routes::webhooks::applications_router())
        .nest("/api/v1/servers", routes::roles::router())
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state)
}

async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

async fn health_ready(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let pg = state.db.check_pg_health().await;
    let scylla = state.db.check_scylla_health().await;
    let redis_ok = check_redis_health(&state).await;
    let nats = state.nats.check_health().await;

    let all_ok = pg && scylla && redis_ok && nats;
    let status = if all_ok { "ok" } else { "degraded" };

    let code = if all_ok {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    };

    (
        code,
        Json(json!({
            "status": status,
            "checks": {
                "postgres": { "status": if pg { "ok" } else { "error" } },
                "scylladb": { "status": if scylla { "ok" } else { "error" } },
                "redis": { "status": if redis_ok { "ok" } else { "error" } },
                "nats": { "status": if nats { "ok" } else { "error" } },
            }
        })),
    )
}

async fn check_redis_health(state: &AppState) -> bool {
    let mut conn = state.redis.clone();
    redis::cmd("PING")
        .query_async::<String>(&mut conn)
        .await
        .is_ok()
}

fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,lumiere=debug,tower_http=debug"));

    let env = std::env::var("LUMIERE_ENV").unwrap_or_default();

    if env == "production" {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer().json())
            .init();
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer().pretty())
            .init();
    }
}
