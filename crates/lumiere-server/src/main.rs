use lumiere_models::config::AppConfig;
use lumiere_server::{build_app_state, build_router};
use tokio_util::sync::CancellationToken;
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

    // 6. Start background JetStream workers
    let cancel = CancellationToken::new();
    let mut worker_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    let worker_state = state.clone();
    let worker_cancel = cancel.clone();
    worker_handles.push(tokio::spawn(async move {
        lumiere_server::workers::search_indexer::start(worker_state, worker_cancel).await;
    }));

    let worker_state = state.clone();
    let worker_cancel = cancel.clone();
    worker_handles.push(tokio::spawn(async move {
        lumiere_server::workers::push_worker::start(worker_state, worker_cancel).await;
    }));

    let worker_state = state.clone();
    let worker_cancel = cancel.clone();
    worker_handles.push(tokio::spawn(async move {
        lumiere_server::workers::read_state_worker::start(worker_state, worker_cancel).await;
    }));

    tracing::info!("Background workers started (search-indexer, push-worker, read-state-updater)");

    // 7. Start server with graceful shutdown
    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("Lumiere server listening on {}", addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(cancel))
        .await?;

    // Wait for all workers to finish draining
    tracing::info!("Waiting for background workers to shut down...");
    for handle in worker_handles {
        let _ = handle.await;
    }

    tracing::info!("Lumiere server shut down cleanly");
    Ok(())
}

/// Wait for SIGINT (Ctrl+C) or SIGTERM, then cancel all workers.
async fn shutdown_signal(cancel: CancellationToken) {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("Received SIGINT, initiating graceful shutdown"),
        _ = terminate => tracing::info!("Received SIGTERM, initiating graceful shutdown"),
    }

    // Signal all workers to stop
    cancel.cancel();

    // Give workers a moment to drain in-flight messages
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
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
