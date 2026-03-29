# Lumiere Server

## What is Lumiere?

Lumiere is a **Discord alternative** — a real-time messaging platform with voice/video, built from scratch in Rust. It is NOT based on Matrix or any existing protocol. It has its own custom protocol, API, and architecture.

**Goal:** Build the most performant, self-hostable, open-source Discord alternative possible.

## Architecture Overview

```
Flutter Client (iOS/Android/Web)
        |
        | REST + WebSocket
        v
[Axum + Tokio Gateway] ── auth ──> [PostgreSQL] (users, servers, channels, roles)
        |
        | publish/subscribe
        v
    [NATS Server]
    /          \
Core PubSub    JetStream
(instant        (persistence, search indexer,
 fanout)         push notification worker)
    |                |
    v                v
[WebSocket       [ScyllaDB] (messages)
 broadcast       [Meilisearch] (search index)
 to clients]     [APNs/FCM] (push notifications)

[LiveKit] ── voice/video (separate WebRTC SFU service)
[MinIO]   ── file/media storage (S3-compatible)
[Redis]   ── cache, presence, rate limiting, sessions
```

## Tech Stack

| Layer | Technology | Purpose |
|-------|-----------|---------|
| API + WebSocket | **Rust + Axum + Tokio** | HTTP REST API + WebSocket gateway |
| Message Broker | **NATS** (Core + JetStream) | Real-time pub/sub fanout + durable streams |
| Message Storage | **ScyllaDB** | Trillion-scale message storage, partition: `(channel_id, bucket)` |
| Metadata Storage | **PostgreSQL** | Users, servers, channels, roles, permissions |
| Cache + Presence | **Redis** | Online status, hot cache, rate limiting, sessions |
| Search | **Meilisearch** | Full-text message search, Rust-native |
| Voice/Video | **LiveKit** | Open-source WebRTC SFU |
| File Storage | **MinIO** | S3-compatible self-hosted object storage |
| Push (iOS) | **APNs** | Direct Apple Push Notification Service |
| Push (Android) | **FCM** | Firebase Cloud Messaging (free tier) |
| ID System | **Snowflake ID** | 64-bit: 41-bit timestamp + 10-bit machine + 12-bit sequence |
| Connection Pool | **PgBouncer** | PostgreSQL connection management |

## Project Structure (Target)

```
lumiere-server/
├── Cargo.toml                  # Workspace root
├── docker-compose.yml          # All services (ScyllaDB, PG, Redis, NATS, etc.)
├── .claude/
│   ├── CLAUDE.md               # This file
│   ├── agents/                 # Claude agent configurations
│   ├── skills/                 # Claude skill definitions
│   └── sprints/                # Sprint planning documents
├── crates/
│   ├── lumiere-server/         # Main binary — Axum server, routes, WebSocket
│   ├── lumiere-gateway/        # WebSocket gateway logic, connection management
│   ├── lumiere-models/         # Shared data types, Snowflake ID, DTOs
│   ├── lumiere-db/             # Database layer — ScyllaDB + PostgreSQL queries
│   ├── lumiere-auth/           # Authentication, JWT, password hashing
│   ├── lumiere-permissions/    # Permission bitfield system, role hierarchy
│   ├── lumiere-nats/           # NATS client wrapper, pub/sub helpers
│   ├── lumiere-search/         # Meilisearch integration, indexing pipeline
│   ├── lumiere-media/          # MinIO/S3 file operations, thumbnails
│   ├── lumiere-push/           # APNs + FCM push notification delivery
│   ├── lumiere-voice/          # LiveKit integration, voice state
│   └── lumiere-data-services/  # Request coalescing, hot partition protection
├── migrations/
│   ├── postgres/               # PostgreSQL migrations (sqlx)
│   └── scylla/                 # ScyllaDB CQL migrations
└── config/
    ├── default.toml            # Default configuration
    ├── development.toml        # Dev overrides
    └── production.toml         # Production overrides
```

## Key Design Decisions

### Snowflake IDs (not UUIDs)
All entities use 64-bit Snowflake IDs. They are time-sortable, half the size of UUIDs (8 bytes vs 16), and provide natural chronological ordering for cursor-based pagination.

### ScyllaDB Message Partitioning
```cql
PRIMARY KEY ((channel_id, bucket), message_id)
```
- `channel_id` + time-based `bucket` = bounded partitions
- `message_id` (Snowflake) = clustering key, natural chronological sort
- Prevents unbounded partition growth in active channels

### NATS Dual-Path
- **Core NATS**: Fire-and-forget pub/sub for instant WebSocket fanout (sub-millisecond)
- **JetStream**: Durable streams for message persistence workers, search indexers, push notification delivery

### Data Services Layer
Rust intermediary between API and databases. Implements request coalescing — when 10,000 users load the same channel simultaneously, only ONE database query executes. Discord's exact pattern.

### Permission Bitfield
Discord-style permission system using 64-bit integers. Each permission is a bit flag. Role permissions are ORed together, channel overrides can allow/deny specific bits.

## Development Conventions

- **Language**: Rust (latest stable)
- **Async runtime**: Tokio
- **Error handling**: `thiserror` for library errors, `anyhow` for application errors
- **Serialization**: `serde` + `serde_json`
- **Database**: `sqlx` for PostgreSQL, `scylla` crate for ScyllaDB
- **Testing**: `tokio::test` for async tests, integration tests with real databases via Docker
- **Config**: `config` crate with TOML files
- **Logging**: `tracing` crate with structured logging
- **Code style**: `cargo fmt` + `cargo clippy` — zero warnings policy

## Running Locally

```bash
# Start all infrastructure
docker-compose up -d

# Run the server
cargo run --bin lumiere-server

# Run tests
cargo test

# Run specific crate tests
cargo test -p lumiere-auth
```

## Sprint Plan

See [sprints/OVERVIEW.md](sprints/OVERVIEW.md) for the complete sprint breakdown.

All **23 sprints** are implemented. Each sprint has its own detailed document in the `sprints/` directory.

### Current Status (2026-03-29)
- **All sprints implemented** (04-23 implemented in one session, 01-03 were pre-existing)
- **Code reviewed**: 130 issues found by 5 parallel review agents, all fixed
- **Build**: 0 errors, 0 warnings, 58 tests passing
- **101 API endpoints** across 10 route modules
- **12 crates**, ~11K lines of Rust

### Known Technical Debt
- ScyllaDB queries use `query_unpaged` instead of prepared statements
- Media crate buffers full files in memory (no streaming)
- Voice state Redis operations are not atomic (read-modify-write race)
- Gateway resume doesn't replay missed events
- Rate limiting middleware not yet implemented (only slowmode exists)
- E2E encryption is schema-only (MLS protocol not implemented)

### Production Deployment
See memory file `deployment_checklist.md` for required env vars. Key guards:
- `LUMIERE_ENV=production` enables CORS lockdown, JWT secret validation, MACHINE_ID requirement
- Server panics on startup if JWT secret is default or MACHINE_ID is missing in production
