pub mod middleware;
pub mod routes;
pub mod workers;

use axum::{
    http::{HeaderName, Method},
    middleware as axum_middleware,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use lumiere_auth::middleware::AuthState;
use lumiere_db::Database;
use lumiere_media::{MediaService, S3Client};
use lumiere_models::config::AppConfig;
use lumiere_nats::NatsService;
use serde_json::json;
use std::sync::Arc;
use tower_http::{
    cors::{Any, CorsLayer},
    limit::RequestBodyLimitLayer,
    trace::TraceLayer,
};

use middleware::rate_limit::global_rate_limit_middleware;
use middleware::security::security_headers_middleware;

pub struct AppState {
    pub config: AppConfig,
    pub db: Database,
    pub redis: redis::aio::ConnectionManager,
    pub nats: NatsService,
    pub snowflake: lumiere_models::snowflake::SnowflakeGenerator,
    pub media: MediaService,
    /// Meilisearch search service. `None` if Meilisearch is not available.
    pub search: Option<lumiere_search::SearchService>,
    /// Push notification service. `None` if push is not configured.
    pub push: Option<lumiere_push::PushService>,
}

impl AuthState for AppState {
    fn jwt_secret(&self) -> &str {
        &self.config.auth.jwt_secret
    }
}

/// Build the full application state from config
pub async fn build_app_state(config: AppConfig) -> anyhow::Result<Arc<AppState>> {
    let mut db = Database::connect(&config).await?;
    db.run_pg_migrations().await?;
    db.run_scylla_migrations(&config).await?;
    db.prepare_all().await?;

    let redis_client = redis::Client::open(config.redis.url.as_str())?;
    let redis = redis::aio::ConnectionManager::new(redis_client).await?;

    let nats = NatsService::connect(&config.nats).await?;
    nats.setup_streams().await?;

    let s3_client = S3Client::connect(&config.minio)
        .map_err(|e| anyhow::anyhow!("Failed to connect to MinIO/S3: {}", e))?;
    let media = MediaService::new(s3_client);

    let machine_id: u16 = std::env::var("MACHINE_ID")
        .unwrap_or_else(|_| "1".into())
        .parse()
        .unwrap_or(1);
    let snowflake = lumiere_models::snowflake::SnowflakeGenerator::new(machine_id);

    // Connect to Meilisearch (best-effort — workers degrade gracefully if absent)
    let search = match lumiere_search::SearchService::connect(&config.meilisearch).await {
        Ok(s) => {
            tracing::info!("Meilisearch connected");
            Some(s)
        }
        Err(e) => {
            tracing::warn!(error = %e, "Meilisearch not available — search indexer will be disabled");
            None
        }
    };

    // Push notification service — PostgreSQL-backed token store, real APNs/FCM
    // clients if credentials are configured, otherwise graceful degradation.
    let push = {
        let token_store = std::sync::Arc::new(lumiere_push::TokenStoreBackend::Postgres(
            lumiere_push::PgDeviceTokenStore::new(db.pg.clone()),
        ));

        let apns = config.push.apns.as_ref().and_then(|cfg| {
            match lumiere_push::ApnsClient::new(
                &cfg.key_path,
                &cfg.key_id,
                &cfg.team_id,
                &cfg.bundle_id,
                cfg.sandbox,
            ) {
                Ok(client) => Some(client),
                Err(e) => {
                    tracing::warn!(error = %e, "APNs client not available — iOS push disabled");
                    None
                }
            }
        });

        let fcm = config.push.fcm.as_ref().and_then(|cfg| {
            match lumiere_push::FcmClient::new(&cfg.service_account_key_path, &cfg.project_id) {
                Ok(client) => Some(client),
                Err(e) => {
                    tracing::warn!(error = %e, "FCM client not available — Android push disabled");
                    None
                }
            }
        });

        Some(lumiere_push::PushService::new(apns, fcm, token_store))
    };

    Ok(Arc::new(AppState {
        config,
        db,
        redis,
        nats,
        snowflake,
        media,
        search,
        push,
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
        .route(
            "/gateway",
            get(lumiere_gateway::handler::ws_upgrade).with_state(gateway_state),
        )
        .nest("/api/v1/auth", routes::auth::router())
        .nest("/api/v1/users", routes::users::router())
        .nest("/api/v1/servers", routes::servers::router())
        .nest("/api/v1/invites", routes::servers::invite_router())
        .nest("/api/v1/channels", routes::servers::channel_invite_router())
        .nest("/api/v1/channels", routes::channels::router())
        .nest(
            "/api/v1/channels",
            routes::roles::channel_permissions_router(),
        )
        .nest("/api/v1/channels", routes::messages::router())
        .nest("/api/v1/channels", routes::reactions::router())
        .nest("/api/v1/channels", routes::typing::router())
        .nest("/api/v1/users", routes::devices::router())
        .nest("/api/v1/users", routes::typing::user_unread_router())
        .nest("/api/v1/servers", routes::typing::server_ack_router())
        .nest("/api/v1/servers", routes::moderation::router())
        .nest("/api/v1", routes::moderation::report_router())
        .nest("/api/v1/channels", routes::webhooks::router())
        .nest("/api/v1/webhooks", routes::webhooks::webhook_exec_router())
        .nest(
            "/api/v1/applications",
            routes::webhooks::applications_router(),
        )
        .nest("/api/v1/servers", routes::roles::router())
        // Attachment upload route with 50 MB body limit
        .nest(
            "/api/v1/channels",
            routes::attachments::upload_router()
                .layer(RequestBodyLimitLayer::new(50 * 1024 * 1024)),
        )
        // Attachment download route (no special body limit needed)
        .nest(
            "/api/v1/attachments",
            routes::attachments::download_router(),
        )
        // Default request body size limit: 1 MB
        .layer(RequestBodyLimitLayer::new(1024 * 1024))
        // Global rate limiting (token bucket via Redis)
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            global_rate_limit_middleware,
        ))
        // Security headers on every response
        .layer(axum_middleware::from_fn(security_headers_middleware))
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state)
}

async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}
