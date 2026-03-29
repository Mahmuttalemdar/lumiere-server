use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub scylla: ScyllaConfig,
    pub redis: RedisConfig,
    pub nats: NatsConfig,
    pub meilisearch: MeilisearchConfig,
    pub minio: MinioConfig,
    pub livekit: LivekitConfig,
    pub auth: AuthConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
    pub min_connections: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScyllaConfig {
    pub nodes: Vec<String>,
    pub keyspace: String,
    pub replication_factor: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RedisConfig {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NatsConfig {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MeilisearchConfig {
    pub url: String,
    pub api_key: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MinioConfig {
    pub endpoint: String,
    pub access_key: String,
    pub secret_key: String,
    pub bucket: String,
    pub region: String,
    pub use_ssl: bool,
    /// Whether to use path-style addressing (e.g. `http://host/bucket/key`
    /// instead of `http://bucket.host/key`). Required for most MinIO setups.
    /// Defaults to `true` when not specified in config.
    #[serde(default = "default_use_path_style")]
    pub use_path_style: bool,
}

fn default_use_path_style() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct LivekitConfig {
    pub url: String,
    pub api_key: String,
    pub api_secret: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthConfig {
    pub jwt_secret: String,
    pub access_token_ttl: u64,
    pub refresh_token_ttl: u64,
}

impl AppConfig {
    pub fn load() -> anyhow::Result<Self> {
        let env = std::env::var("LUMIERE_ENV").unwrap_or_else(|_| "development".into());

        let config = config::Config::builder()
            .add_source(config::File::with_name("config/default"))
            .add_source(config::File::with_name(&format!("config/{}", env)).required(false))
            .add_source(
                config::Environment::with_prefix("LUMIERE")
                    .separator("__")
                    .try_parsing(true),
            )
            .build()?;

        Ok(config.try_deserialize()?)
    }
}
