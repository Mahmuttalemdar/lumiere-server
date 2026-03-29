# Lumiere Server

Real-time messaging platform backend — a Discord alternative built from scratch in Rust.

## Tech Stack

| Layer | Technology |
|-------|-----------|
| API + WebSocket | Rust, Axum, Tokio |
| Message Broker | NATS (Core + JetStream) |
| Message Storage | ScyllaDB |
| Metadata Storage | PostgreSQL |
| Cache + Presence | Redis |
| Search | Meilisearch |
| Voice/Video | LiveKit |
| File Storage | MinIO (S3-compatible) |
| Push | APNs + FCM |

## Architecture

```
Flutter Client (iOS/Android/Web)
        |
        | REST + WebSocket
        v
   [Axum Gateway] ── auth ──> [PostgreSQL]
        |
        | pub/sub
        v
    [NATS Server]
    /          \
Core PubSub    JetStream
    |              |
    v              v
[WebSocket     [ScyllaDB]
 fanout]       [Meilisearch]
               [APNs/FCM]
```

## Quick Start

```bash
# Start infrastructure
docker-compose up -d

# Run server
cargo run --bin lumiere-server

# Run tests
cargo test
```

## Project Structure

```
lumiere-server/
├── crates/
│   ├── lumiere-server/         # Main binary — Axum routes, router
│   ├── lumiere-gateway/        # WebSocket gateway, NATS event fanout
│   ├── lumiere-models/         # Snowflake ID, config, error types
│   ├── lumiere-db/             # PostgreSQL + ScyllaDB connections
│   ├── lumiere-auth/           # JWT, Argon2, sessions, middleware
│   ├── lumiere-permissions/    # Discord-style permission bitfields
│   ├── lumiere-nats/           # NATS client wrapper
│   ├── lumiere-media/          # MinIO/S3 file operations
│   ├── lumiere-search/         # Meilisearch integration
│   ├── lumiere-voice/          # LiveKit voice/video
│   ├── lumiere-push/           # APNs + FCM push notifications
│   └── lumiere-data-services/  # Request coalescing, hot cache
├── migrations/
│   ├── postgres/               # PostgreSQL schema
│   └── scylla/                 # ScyllaDB CQL schema
├── config/                     # TOML configuration files
├── Dockerfile                  # Multi-stage production build
└── docker-compose.yml          # Local development services
```

## License

Proprietary. Copyright (c) 2024-2026 Alemdar Labs. All rights reserved.
