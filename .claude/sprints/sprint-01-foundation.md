# Sprint 01 — Foundation & Infrastructure

**Status:** Not Started
**Dependencies:** None
**Crates:** workspace root, lumiere-server, lumiere-models

## Goal

Set up the Rust workspace, Docker infrastructure for all services, basic Axum server, configuration system, error handling, and logging. After this sprint, `cargo run` starts a server and `docker-compose up` starts all infrastructure.

## Tasks

### 1.1 — Cargo Workspace Setup

Create a Cargo workspace with all planned crates. Not all crates need implementation yet — just the workspace structure with empty `lib.rs` files.

```toml
# Cargo.toml (workspace root)
[workspace]
resolver = "2"
members = [
    "crates/lumiere-server",
    "crates/lumiere-gateway",
    "crates/lumiere-models",
    "crates/lumiere-db",
    "crates/lumiere-auth",
    "crates/lumiere-permissions",
    "crates/lumiere-nats",
    "crates/lumiere-search",
    "crates/lumiere-media",
    "crates/lumiere-push",
    "crates/lumiere-voice",
    "crates/lumiere-data-services",
]

[workspace.dependencies]
# Shared dependencies — all crates reference these
tokio = { version = "1", features = ["full"] }
axum = { version = "0.8", features = ["ws"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
thiserror = "2"
anyhow = "1"
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
config = "0.14"
sqlx = { version = "0.8", features = ["runtime-tokio", "postgres", "chrono"] }
scylla = "0.15"
redis = { version = "0.27", features = ["tokio-comp", "connection-manager"] }
async-nats = "0.38"
tower = "0.5"
tower-http = { version = "0.6", features = ["cors", "trace", "compression-gzip"] }
```

### 1.2 — Docker Compose

All infrastructure services:

```yaml
# docker-compose.yml
services:
  scylladb:
    image: scylladb/scylla:latest
    ports:
      - "9042:9042"
    volumes:
      - scylla_data:/var/lib/scylla
    command: --smp 2 --memory 1G --overprovisioned 1

  postgres:
    image: postgres:17
    ports:
      - "5432:5432"
    environment:
      POSTGRES_DB: lumiere
      POSTGRES_USER: lumiere
      POSTGRES_PASSWORD: lumiere_dev
    volumes:
      - postgres_data:/var/lib/postgresql/data

  redis:
    image: redis:7-alpine
    ports:
      - "6379:6379"
    volumes:
      - redis_data:/data

  nats:
    image: nats:latest
    ports:
      - "4222:4222"   # Client
      - "8222:8222"   # Monitoring
    command: ["--jetstream", "--store_dir", "/data"]
    volumes:
      - nats_data:/data

  meilisearch:
    image: getmeili/meilisearch:latest
    ports:
      - "7700:7700"
    environment:
      MEILI_MASTER_KEY: lumiere_dev_key
    volumes:
      - meilisearch_data:/meili_data

  minio:
    image: minio/minio:latest
    ports:
      - "9000:9000"
      - "9001:9001"  # Console
    environment:
      MINIO_ROOT_USER: lumiere
      MINIO_ROOT_PASSWORD: lumiere_dev
    command: server /data --console-address ":9001"
    volumes:
      - minio_data:/data

  livekit:
    image: livekit/livekit-server:latest
    ports:
      - "7880:7880"   # HTTP
      - "7881:7881"   # WebSocket
      - "7882:7882/udp" # WebRTC/UDP
    environment:
      LIVEKIT_KEYS: "devkey: devsecret"
    command: --dev

  pgbouncer:
    image: edoburu/pgbouncer:latest
    ports:
      - "6432:6432"
    environment:
      DATABASE_URL: postgres://lumiere:lumiere_dev@postgres:5432/lumiere
      POOL_MODE: transaction
      MAX_CLIENT_CONN: 1000
      DEFAULT_POOL_SIZE: 50
    depends_on:
      - postgres

volumes:
  scylla_data:
  postgres_data:
  redis_data:
  nats_data:
  meilisearch_data:
  minio_data:
```

### 1.3 — Configuration System

Use `config` crate with TOML files + environment variable overrides.

```
config/
├── default.toml        # All defaults
├── development.toml    # Dev-specific overrides
└── production.toml     # Production overrides
```

Configuration struct:

```rust
// crates/lumiere-models/src/config.rs
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

pub struct ServerConfig {
    pub host: String,        // 0.0.0.0
    pub port: u16,           // 8080
    pub workers: usize,      // num_cpus
}

pub struct DatabaseConfig {
    pub url: String,         // postgres://...
    pub max_connections: u32, // 50
    pub min_connections: u32, // 5
}

pub struct ScyllaConfig {
    pub nodes: Vec<String>,  // ["127.0.0.1:9042"]
    pub keyspace: String,    // lumiere
    pub replication_factor: u32, // 1 for dev, 3 for prod
}

pub struct RedisConfig {
    pub url: String,         // redis://127.0.0.1:6379
}

pub struct NatsConfig {
    pub url: String,         // nats://127.0.0.1:4222
}

pub struct AuthConfig {
    pub jwt_secret: String,
    pub access_token_ttl: u64,   // seconds, default 900 (15 min)
    pub refresh_token_ttl: u64,  // seconds, default 604800 (7 days)
}
```

### 1.4 — Axum Server Skeleton

```rust
// crates/lumiere-server/src/main.rs
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Load config
    // 2. Initialize tracing
    // 3. Connect to all services (PG, ScyllaDB, Redis, NATS)
    // 4. Build Axum router with shared AppState
    // 5. Start server
}
```

AppState holds all connection pools:

```rust
pub struct AppState {
    pub config: AppConfig,
    pub pg_pool: sqlx::PgPool,
    pub scylla: scylla::Session,
    pub redis: redis::aio::ConnectionManager,
    pub nats: async_nats::Client,
    pub jetstream: async_nats::jetstream::Context,
}
```

### 1.5 — Error Handling Framework

Global error type with Axum `IntoResponse`:

```rust
// crates/lumiere-models/src/error.rs
pub enum AppError {
    // Auth errors
    Unauthorized(String),
    Forbidden(String),

    // Resource errors
    NotFound(String),
    AlreadyExists(String),

    // Validation errors
    ValidationError(Vec<FieldError>),

    // Rate limiting
    RateLimited { retry_after: u64 },

    // Internal
    Internal(anyhow::Error),
    Database(String),
    ServiceUnavailable(String),
}
```

Each variant maps to an HTTP status code and JSON error response:

```json
{
    "error": {
        "code": "UNAUTHORIZED",
        "message": "Invalid or expired token"
    }
}
```

### 1.6 — Structured Logging

Use `tracing` with JSON output in production, pretty output in development:

```rust
// Development: colored, human-readable
// Production: JSON with fields (request_id, user_id, latency_ms)
```

Every request gets a unique `request_id` via middleware. All log entries within that request carry the ID.

### 1.7 — Health Check Endpoints

```
GET /health          → 200 OK (basic liveness)
GET /health/ready    → 200 OK if all services connected, 503 otherwise
```

Ready check pings: PostgreSQL, ScyllaDB, Redis, NATS.

## Acceptance Criteria

- [ ] `docker-compose up -d` starts all 8 services without errors
- [ ] `cargo build` compiles the entire workspace
- [ ] `cargo run --bin lumiere-server` starts and connects to all services
- [ ] `GET /health` returns 200
- [ ] `GET /health/ready` returns 200 when all services are up, 503 when any is down
- [ ] Logs are structured JSON with request_id
- [ ] Configuration loads from TOML files with env var overrides
- [ ] All crate skeletons exist in workspace (empty `lib.rs` files)
