# Sprint 21 — Monitoring & Observability

**Status:** Not Started
**Dependencies:** Sprint 01
**Crates:** All crates

## Goal

Full observability: Prometheus metrics, structured logging with tracing, OpenTelemetry distributed traces, Grafana dashboards, health checks, and alerting.

## Tasks

### 21.1 — Prometheus Metrics

Use `metrics` and `metrics-exporter-prometheus` crates:

```
GET /metrics    → Prometheus scrape endpoint
```

**Application Metrics:**

```rust
// HTTP layer
http_requests_total{method, path, status}           // Counter
http_request_duration_seconds{method, path}          // Histogram
http_request_size_bytes{method, path}                // Histogram
http_response_size_bytes{method, path}               // Histogram

// WebSocket layer
ws_connections_active                                 // Gauge
ws_connections_total                                  // Counter
ws_messages_sent_total{event_type}                   // Counter
ws_messages_received_total{op_code}                  // Counter

// Database layer
db_query_duration_seconds{database, operation}       // Histogram
db_connections_active{database}                      // Gauge
db_connections_idle{database}                        // Gauge
db_errors_total{database, operation}                 // Counter

// NATS layer
nats_messages_published_total{subject}               // Counter
nats_messages_received_total{subject}                // Counter
nats_publish_duration_seconds                        // Histogram

// Cache layer
cache_hits_total{cache_type}                         // Counter
cache_misses_total{cache_type}                       // Counter
cache_coalesce_hits_total                            // Counter (request coalescing)

// Business metrics
messages_sent_total                                   // Counter
users_registered_total                                // Counter
servers_created_total                                 // Counter
voice_sessions_active                                 // Gauge
push_notifications_sent_total{platform, status}      // Counter
search_queries_total                                  // Counter
search_query_duration_seconds                        // Histogram
```

### 21.2 — Metrics Middleware

Axum middleware that automatically records HTTP metrics:

```rust
pub struct MetricsLayer;

impl<S> Layer<S> for MetricsLayer {
    type Service = MetricsService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        MetricsService { inner }
    }
}

// Records: request count, duration, status code, method, path
```

### 21.3 — Structured Logging with Tracing

Already partially set up in Sprint 01. Expand:

```rust
// Every request gets a span with:
#[instrument(
    skip(state, body),
    fields(
        request_id = %request_id,
        user_id = %auth.id,
        method = %method,
        path = %path,
    )
)]
async fn handler(...) { ... }
```

Log levels:
- **ERROR**: Database failures, NATS connection loss, unrecoverable errors
- **WARN**: Rate limit hits, failed auth attempts, degraded service
- **INFO**: Request start/end, significant business events (user register, server create)
- **DEBUG**: Database queries, NATS messages, cache hits/misses
- **TRACE**: Full request/response bodies (dev only)

JSON log format (production):
```json
{
    "timestamp": "2026-03-29T12:00:00Z",
    "level": "INFO",
    "target": "lumiere_server::handlers::messages",
    "message": "Message sent",
    "request_id": "abc-123",
    "user_id": "456",
    "channel_id": "789",
    "message_id": "012",
    "latency_ms": 12
}
```

### 21.4 — OpenTelemetry Distributed Tracing

```rust
// Trace propagation across services
// API → NATS → Worker (push, search, etc.)

use opentelemetry::sdk::trace::TracerProvider;
use tracing_opentelemetry::OpenTelemetryLayer;

pub fn setup_tracing(config: &AppConfig) -> Result<()> {
    let tracer = opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(opentelemetry_otlp::new_exporter().tonic())
        .build_simple()?;

    let telemetry = OpenTelemetryLayer::new(tracer);

    tracing_subscriber::registry()
        .with(telemetry)
        .with(fmt_layer)
        .with(EnvFilter::from_default_env())
        .init();

    Ok(())
}
```

Inject trace context into NATS messages:
```rust
// When publishing to NATS, inject trace headers
let mut headers = async_nats::HeaderMap::new();
inject_trace_context(&mut headers);
client.publish_with_headers(subject, headers, payload).await?;
```

### 21.5 — Health Check Endpoints

Expand from Sprint 01:

```
GET /health
    Response: { status: "ok" }

GET /health/ready
    Response: {
        status: "ok"|"degraded"|"unhealthy",
        checks: {
            postgres: { status: "ok", latency_ms: 2 },
            scylladb: { status: "ok", latency_ms: 5 },
            redis: { status: "ok", latency_ms: 1 },
            nats: { status: "ok", latency_ms: 1 },
            meilisearch: { status: "ok", latency_ms: 8 },
            minio: { status: "ok", latency_ms: 3 },
        }
    }

GET /health/startup
    Response: { status: "ok" }
    Note: Returns 200 only after initial setup is complete (migrations, etc.)
```

### 21.6 — Grafana Dashboards

Docker Compose includes Grafana + Prometheus:

```yaml
# Add to docker-compose.yml
prometheus:
    image: prom/prometheus:latest
    ports:
      - "9090:9090"
    volumes:
      - ./config/prometheus.yml:/etc/prometheus/prometheus.yml

grafana:
    image: grafana/grafana:latest
    ports:
      - "3000:3000"
    environment:
      GF_SECURITY_ADMIN_PASSWORD: admin
    volumes:
      - grafana_data:/var/lib/grafana
      - ./config/grafana/dashboards:/etc/grafana/provisioning/dashboards
```

Dashboard panels:
1. **Overview**: Request rate, error rate, p50/p95/p99 latency
2. **WebSocket**: Active connections, events/sec, connection lifetime
3. **Database**: Query latency per DB, connection pool usage, error rate
4. **Cache**: Hit rate, coalesce rate, memory usage
5. **NATS**: Message rate, consumer lag, JetStream pending
6. **Business**: Messages/sec, active users, voice sessions, registrations

### 21.7 — Alerting Rules

Prometheus alerting rules:

```yaml
# config/prometheus/alerts.yml
groups:
  - name: lumiere
    rules:
      - alert: HighErrorRate
        expr: rate(http_requests_total{status=~"5.."}[5m]) / rate(http_requests_total[5m]) > 0.05
        for: 5m
        labels:
          severity: critical

      - alert: HighLatency
        expr: histogram_quantile(0.99, rate(http_request_duration_seconds_bucket[5m])) > 1
        for: 5m
        labels:
          severity: warning

      - alert: DatabaseDown
        expr: up{job="scylladb"} == 0 or up{job="postgres"} == 0
        for: 1m
        labels:
          severity: critical

      - alert: HighMemoryUsage
        expr: process_resident_memory_bytes > 2e9
        for: 10m
        labels:
          severity: warning

      - alert: WebSocketConnectionsDrop
        expr: delta(ws_connections_active[5m]) < -1000
        for: 1m
        labels:
          severity: critical

      - alert: NATSConsumerLag
        expr: nats_consumer_pending > 10000
        for: 5m
        labels:
          severity: warning
```

## Acceptance Criteria

- [ ] Prometheus metrics endpoint exposes all defined metrics
- [ ] HTTP metrics middleware records request count, duration, status
- [ ] WebSocket metrics track active connections and event throughput
- [ ] Database query latency tracked per operation
- [ ] Cache hit/miss rates recorded
- [ ] Structured JSON logs with request_id correlation
- [ ] OpenTelemetry traces propagated across NATS messages
- [ ] Health check endpoints return correct status per service
- [ ] Grafana dashboards provisioned with useful panels
- [ ] Alerting rules configured for critical conditions
- [ ] Trace spans visible in Jaeger/Tempo (or similar)
- [ ] Zero performance impact from metrics (< 1% overhead)
