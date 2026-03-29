use lumiere_models::config::AppConfig;
use scylla::Session;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct Database {
    pub pg: PgPool,
    pub scylla: Arc<Session>,
}

impl Database {
    pub async fn connect(config: &AppConfig) -> anyhow::Result<Self> {
        let pg = PgPoolOptions::new()
            .max_connections(config.database.max_connections)
            .min_connections(config.database.min_connections)
            .acquire_timeout(Duration::from_secs(3))
            .idle_timeout(Duration::from_secs(600))
            .max_lifetime(Duration::from_secs(1800))
            .connect(&config.database.url)
            .await?;

        tracing::info!("Connected to PostgreSQL");

        let scylla = scylla::SessionBuilder::new()
            .known_nodes(&config.scylla.nodes)
            .build()
            .await?;

        tracing::info!("Connected to ScyllaDB");

        Ok(Self {
            pg,
            scylla: Arc::new(scylla),
        })
    }

    pub async fn run_pg_migrations(&self) -> anyhow::Result<()> {
        tracing::info!("Running PostgreSQL migrations...");
        sqlx::migrate!("../../migrations/postgres")
            .run(&self.pg)
            .await?;
        tracing::info!("PostgreSQL migrations complete");
        Ok(())
    }

    pub async fn run_scylla_migrations(&self, config: &AppConfig) -> anyhow::Result<()> {
        tracing::info!("Running ScyllaDB migrations...");

        // Create keyspace if not exists
        let create_keyspace = format!(
            "CREATE KEYSPACE IF NOT EXISTS {} WITH replication = {{'class': 'SimpleStrategy', 'replication_factor': {}}}",
            config.scylla.keyspace, config.scylla.replication_factor
        );
        self.scylla
            .query_unpaged(create_keyspace, &[])
            .await?;

        self.scylla
            .use_keyspace(&config.scylla.keyspace, false)
            .await?;

        // Read and execute .cql migration files in order
        let mut entries: Vec<_> = std::fs::read_dir("migrations/scylla")?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .is_some_and(|ext| ext == "cql")
            })
            .collect();
        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            let content = std::fs::read_to_string(entry.path())?;
            for statement in content.split(';') {
                let stmt = statement.trim();
                if stmt.is_empty() || stmt.starts_with("--") {
                    continue;
                }
                // Skip USE statements (we already set keyspace)
                if stmt.to_uppercase().starts_with("USE ") {
                    continue;
                }
                // Skip CREATE KEYSPACE (already handled above)
                if stmt.to_uppercase().starts_with("CREATE KEYSPACE") {
                    continue;
                }
                tracing::debug!(statement = stmt, "Executing CQL migration");
                self.scylla
                    .query_unpaged(stmt.to_string(), &[])
                    .await
                    .map_err(|e| {
                        anyhow::anyhow!("CQL migration failed: {} — statement: {}", e, stmt)
                    })?;
            }
            tracing::info!(file = %entry.file_name().to_string_lossy(), "CQL migration applied");
        }

        tracing::info!("ScyllaDB migrations complete");
        Ok(())
    }

    pub async fn check_pg_health(&self) -> bool {
        sqlx::query("SELECT 1")
            .execute(&self.pg)
            .await
            .is_ok()
    }

    pub async fn check_scylla_health(&self) -> bool {
        self.scylla
            .query_unpaged("SELECT now() FROM system.local", &[])
            .await
            .is_ok()
    }
}
