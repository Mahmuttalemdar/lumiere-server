use lumiere_models::config::AppConfig;
use lumiere_server::{build_app_state, build_router};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

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

    // 3. Validate production settings
    if std::env::var("LUMIERE_ENV").unwrap_or_default() == "production" {
        std::env::var("MACHINE_ID")
            .expect("MACHINE_ID must be set in production");
        if config.auth.jwt_secret == "lumiere_dev_jwt_secret_change_in_production" {
            panic!("JWT secret must be changed in production! Set LUMIERE__AUTH__JWT_SECRET");
        }
    }

    // 4. Build app state (connects to all services, runs migrations)
    let state = build_app_state(config.clone()).await?;

    // 5. Build router
    let mut app = build_router(state.clone());

    // Add metrics endpoint (not in lib.rs — production only concern)
    let metrics_handle = prometheus_handle;
    app = app.route("/metrics", axum::routing::get(move || {
        let handle = metrics_handle.clone();
        async move { handle.render() }
    }));

    // Add production CORS override
    if std::env::var("LUMIERE_ENV").unwrap_or_default() == "production" {
        use tower_http::cors::CorsLayer;
        let cors = CorsLayer::new()
            .allow_origin("https://app.lumiere.chat".parse::<axum::http::HeaderValue>().unwrap())
            .allow_methods([
                axum::http::Method::GET,
                axum::http::Method::POST,
                axum::http::Method::PATCH,
                axum::http::Method::PUT,
                axum::http::Method::DELETE,
            ])
            .allow_credentials(true);
        app = app.layer(cors);
    }

    // 6. Start server
    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("Lumiere server listening on {}", addr);

    axum::serve(listener, app).await?;

    Ok(())
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
