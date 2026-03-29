# Sprint 23 — Deployment & DevOps

**Status:** Not Started
**Dependencies:** Sprint 21
**Crates:** Infrastructure

## Goal

Production-ready deployment: optimized Docker images, production Docker Compose, TLS/SSL, reverse proxy, backup strategy, CI/CD pipeline, and rolling deployment.

## Tasks

### 23.1 — Production Docker Image

Multi-stage build for minimal image size:

```dockerfile
# Dockerfile
# Stage 1: Build
FROM rust:1.78-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release --bin lumiere-server

# Stage 2: Runtime
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/lumiere-server /usr/local/bin/
EXPOSE 8080
CMD ["lumiere-server"]
```

Target image size: < 50 MB.

### 23.2 — Production Docker Compose

```yaml
# docker-compose.prod.yml
services:
  lumiere:
    image: lumiere-server:latest
    restart: always
    environment:
      LUMIERE_ENV: production
      LUMIERE_DATABASE_URL: postgres://...
      LUMIERE_JWT_SECRET: ${JWT_SECRET}
    deploy:
      replicas: 2
      resources:
        limits:
          cpus: '2'
          memory: 1G

  caddy:
    image: caddy:2-alpine
    restart: always
    ports:
      - "80:80"
      - "443:443"
    volumes:
      - ./Caddyfile:/etc/caddy/Caddyfile
      - caddy_data:/data

  scylladb:
    image: scylladb/scylla:latest
    restart: always
    command: --smp 4 --memory 4G --overprovisioned 1
    volumes:
      - scylla_data:/var/lib/scylla
    deploy:
      resources:
        limits:
          cpus: '4'
          memory: 5G

  postgres:
    image: postgres:17
    restart: always
    environment:
      POSTGRES_DB: lumiere
      POSTGRES_USER: ${PG_USER}
      POSTGRES_PASSWORD: ${PG_PASSWORD}
    volumes:
      - postgres_data:/var/lib/postgresql/data
      - ./config/postgres/postgresql.conf:/etc/postgresql/postgresql.conf
    command: postgres -c config_file=/etc/postgresql/postgresql.conf

  pgbouncer:
    image: edoburu/pgbouncer:latest
    restart: always
    environment:
      DATABASE_URL: postgres://${PG_USER}:${PG_PASSWORD}@postgres:5432/lumiere
      POOL_MODE: transaction
      MAX_CLIENT_CONN: 2000
      DEFAULT_POOL_SIZE: 100

  redis:
    image: redis:7-alpine
    restart: always
    command: redis-server --maxmemory 512mb --maxmemory-policy allkeys-lru --appendonly yes
    volumes:
      - redis_data:/data

  nats:
    image: nats:latest
    restart: always
    command: ["--jetstream", "--store_dir", "/data", "--max_mem_store", "1G", "--max_file_store", "10G"]
    volumes:
      - nats_data:/data

  meilisearch:
    image: getmeili/meilisearch:latest
    restart: always
    environment:
      MEILI_MASTER_KEY: ${MEILI_KEY}
      MEILI_ENV: production
    volumes:
      - meilisearch_data:/meili_data

  minio:
    image: minio/minio:latest
    restart: always
    environment:
      MINIO_ROOT_USER: ${MINIO_USER}
      MINIO_ROOT_PASSWORD: ${MINIO_PASSWORD}
    command: server /data --console-address ":9001"
    volumes:
      - minio_data:/data

  livekit:
    image: livekit/livekit-server:latest
    restart: always
    ports:
      - "7882:7882/udp"
    volumes:
      - ./config/livekit.yaml:/etc/livekit.yaml
    command: --config /etc/livekit.yaml

  prometheus:
    image: prom/prometheus:latest
    restart: always
    volumes:
      - ./config/prometheus.yml:/etc/prometheus/prometheus.yml
      - prometheus_data:/prometheus

  grafana:
    image: grafana/grafana:latest
    restart: always
    volumes:
      - grafana_data:/var/lib/grafana
      - ./config/grafana/:/etc/grafana/provisioning/
```

### 23.3 — TLS / Reverse Proxy (Caddy)

Caddy auto-provisions TLS certificates via Let's Encrypt:

```
# Caddyfile
api.lumiere.app {
    reverse_proxy lumiere:8080

    # WebSocket upgrade
    @websocket {
        header Connection *Upgrade*
        header Upgrade websocket
    }
    reverse_proxy @websocket lumiere:8080
}

cdn.lumiere.app {
    reverse_proxy minio:9000
    header Cache-Control "public, max-age=31536000, immutable"
}

livekit.lumiere.app {
    reverse_proxy livekit:7880
}
```

### 23.4 — PostgreSQL Production Tuning

```
# config/postgres/postgresql.conf
shared_buffers = 1GB
effective_cache_size = 3GB
work_mem = 16MB
maintenance_work_mem = 256MB
max_connections = 200
wal_buffers = 64MB
max_wal_size = 2GB
checkpoint_completion_target = 0.9
random_page_cost = 1.1
effective_io_concurrency = 200
```

### 23.5 — Backup Strategy

**PostgreSQL:**
```bash
# Daily full backup + WAL archiving for point-in-time recovery
pg_dump lumiere | gzip > /backups/pg/lumiere_$(date +%Y%m%d).sql.gz

# WAL archiving
archive_mode = on
archive_command = 'cp %p /backups/pg/wal/%f'
```

**ScyllaDB:**
```bash
# ScyllaDB snapshot
nodetool snapshot lumiere
# Copy snapshot files to backup storage
```

**Redis:**
```bash
# Redis RDB snapshot (already enabled with appendonly)
# Copy appendonly.aof to backup storage
```

**MinIO:**
```bash
# MinIO client (mc) mirror to backup storage
mc mirror minio/lumiere /backups/minio/
```

Backup schedule:
- PostgreSQL: daily full + continuous WAL
- ScyllaDB: daily snapshot
- Redis: hourly RDB
- MinIO: daily mirror
- Retention: 30 days

### 23.6 — CI/CD Pipeline

```yaml
# .github/workflows/ci.yml
name: CI/CD

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo fmt --check
      - run: cargo clippy -- -D warnings
      - run: cargo build --release

  test:
    runs-on: ubuntu-latest
    services:
      postgres:
        image: postgres:17
        env:
          POSTGRES_DB: lumiere_test
          POSTGRES_USER: test
          POSTGRES_PASSWORD: test
        ports: ["5432:5432"]
      scylladb:
        image: scylladb/scylla:latest
        ports: ["9042:9042"]
      redis:
        image: redis:7-alpine
        ports: ["6379:6379"]
      nats:
        image: nats:latest
        ports: ["4222:4222"]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo test --all

  benchmark:
    runs-on: ubuntu-latest
    if: github.event_name == 'pull_request'
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo bench --bench '*' -- --output-format bencher | tee output.txt
      # Compare with base branch

  deploy:
    needs: [check, test]
    if: github.ref == 'refs/heads/main'
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Build Docker image
        run: docker build -t lumiere-server:${{ github.sha }} .
      - name: Push to registry
        run: |
          docker tag lumiere-server:${{ github.sha }} registry/lumiere-server:latest
          docker push registry/lumiere-server:latest
      - name: Deploy
        run: |
          ssh deploy@server "cd /opt/lumiere && docker-compose pull && docker-compose up -d --no-deps lumiere"
```

### 23.7 — Rolling Deployment

Zero-downtime deployment strategy:
1. Build new Docker image
2. Push to registry
3. Pull on production server
4. `docker-compose up -d --no-deps lumiere` (only restarts the app, not infrastructure)
5. Caddy automatically routes to healthy instances
6. Health check confirms new instance is ready

For WebSocket connections:
- Old connections stay on old instance until it shuts down
- Clients reconnect to new instance via Resume (Sprint 08)
- Grace period: 30 seconds for old instance to drain

### 23.8 — Environment Configuration

```bash
# .env.production (not in git)
JWT_SECRET=<generated>
PG_USER=lumiere
PG_PASSWORD=<generated>
MEILI_KEY=<generated>
MINIO_USER=lumiere
MINIO_PASSWORD=<generated>
LIVEKIT_API_KEY=<generated>
LIVEKIT_API_SECRET=<generated>
APNS_KEY_PATH=/certs/apns_key.p8
APNS_KEY_ID=<from apple>
APNS_TEAM_ID=<from apple>
FCM_SERVICE_ACCOUNT=/certs/fcm_service_account.json
```

### 23.9 — Security Hardening

- All inter-service communication on Docker internal network (not exposed)
- Only Caddy (80/443) and LiveKit UDP (7882) exposed to internet
- Redis, PostgreSQL, ScyllaDB, NATS, MinIO, Meilisearch: internal only
- Docker secrets for sensitive values
- Non-root container user
- Read-only filesystem where possible

### 23.10 — Server Sizing Guide

| Users | Server Spec | Estimated Cost |
|-------|------------|---------------|
| 0-1K | 2 vCPU, 4 GB RAM, 40 GB SSD | ~€5/mo (Hetzner CX22) |
| 1K-10K | 4 vCPU, 8 GB RAM, 80 GB SSD | ~€15/mo (Hetzner CX32) |
| 10K-50K | 8 vCPU, 16 GB RAM, 160 GB SSD | ~€30/mo (Hetzner CX42) |
| 50K-100K | 16 vCPU, 32 GB RAM, 320 GB SSD | ~€60/mo |
| 100K+ | Multi-node deployment | Custom |

## Acceptance Criteria

- [ ] Docker image builds and runs in production mode
- [ ] Docker image size < 50 MB
- [ ] TLS works with auto-provisioned certificates
- [ ] All services start with `docker-compose -f docker-compose.prod.yml up`
- [ ] Zero-downtime deployment via rolling restart
- [ ] WebSocket clients reconnect seamlessly after deploy
- [ ] Backup scripts work for all databases
- [ ] CI pipeline: fmt, clippy, test, build on every PR
- [ ] CD pipeline: auto-deploy to production on main merge
- [ ] Inter-service communication not exposed to internet
- [ ] Health checks pass after deployment
- [ ] Monitoring dashboards show healthy system after deploy
