# Stage 1: Build
FROM rust:1.82-slim-bookworm AS builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Cache dependencies
COPY Cargo.toml Cargo.lock ./
COPY crates/lumiere-server/Cargo.toml crates/lumiere-server/Cargo.toml
COPY crates/lumiere-gateway/Cargo.toml crates/lumiere-gateway/Cargo.toml
COPY crates/lumiere-models/Cargo.toml crates/lumiere-models/Cargo.toml
COPY crates/lumiere-db/Cargo.toml crates/lumiere-db/Cargo.toml
COPY crates/lumiere-auth/Cargo.toml crates/lumiere-auth/Cargo.toml
COPY crates/lumiere-permissions/Cargo.toml crates/lumiere-permissions/Cargo.toml
COPY crates/lumiere-nats/Cargo.toml crates/lumiere-nats/Cargo.toml
COPY crates/lumiere-search/Cargo.toml crates/lumiere-search/Cargo.toml
COPY crates/lumiere-media/Cargo.toml crates/lumiere-media/Cargo.toml
COPY crates/lumiere-push/Cargo.toml crates/lumiere-push/Cargo.toml
COPY crates/lumiere-voice/Cargo.toml crates/lumiere-voice/Cargo.toml
COPY crates/lumiere-data-services/Cargo.toml crates/lumiere-data-services/Cargo.toml

# Create dummy src files to build dependencies
RUN mkdir -p crates/lumiere-server/src && echo "fn main() {}" > crates/lumiere-server/src/main.rs
RUN for crate in gateway models db auth permissions nats search media push voice data-services; do \
      mkdir -p crates/lumiere-$crate/src && echo "" > crates/lumiere-$crate/src/lib.rs; \
    done

RUN cargo build --release --bin lumiere-server 2>/dev/null || true

# Now copy real source
COPY . .
RUN cargo build --release --bin lumiere-server

# Stage 2: Runtime
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates curl && rm -rf /var/lib/apt/lists/*

RUN useradd -r -s /bin/false lumiere

COPY --from=builder /app/target/release/lumiere-server /usr/local/bin/lumiere-server
COPY --from=builder /app/config /etc/lumiere/config
COPY --from=builder /app/migrations /etc/lumiere/migrations

USER lumiere

ENV LUMIERE_ENV=production
ENV CONFIG_DIR=/etc/lumiere/config

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

CMD ["lumiere-server"]
