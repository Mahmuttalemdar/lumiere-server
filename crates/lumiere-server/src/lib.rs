pub mod routes;

use axum::{
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

/// Build the full application state from config
pub async fn build_app_state(config: AppConfig) -> anyhow::Result<Arc<AppState>> {
    let db = Database::connect(&config).await?;
    db.run_pg_migrations().await?;
    db.run_scylla_migrations(&config).await?;

    let redis_client = redis::Client::open(config.redis.url.as_str())?;
    let redis = redis::aio::ConnectionManager::new(redis_client).await?;

    let nats = NatsService::connect(&config.nats).await?;
    nats.setup_streams().await?;

    let machine_id: u16 = std::env::var("MACHINE_ID")
        .unwrap_or_else(|_| "1".into())
        .parse()
        .unwrap_or(1);
    let snowflake = lumiere_models::snowflake::SnowflakeGenerator::new(machine_id);

    Ok(Arc::new(AppState {
        config,
        db,
        redis,
        nats,
        snowflake,
    }))
}

/// Build the Axum router (without metrics — for testing)
pub fn build_router(state: Arc<AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::PUT,
            Method::DELETE,
        ])
        .allow_headers([
            axum::http::header::AUTHORIZATION,
            axum::http::header::CONTENT_TYPE,
            axum::http::header::ACCEPT,
            HeaderName::from_static("x-audit-log-reason"),
        ])
        .expose_headers([
            HeaderName::from_static("x-ratelimit-limit"),
            HeaderName::from_static("x-ratelimit-remaining"),
            HeaderName::from_static("x-ratelimit-reset"),
            HeaderName::from_static("retry-after"),
        ]);

    let gateway_state = Arc::new(lumiere_gateway::handler::GatewayState {
        session_manager: lumiere_gateway::session::SessionManager::new(),
        nats: state.nats.clone(),
        redis: state.redis.clone(),
        jwt_secret: state.config.auth.jwt_secret.clone(),
        db: state.db.clone(),
    });

    Router::new()
        .route("/health", get(health))
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
