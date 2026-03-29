# Load Tests

Requires [k6](https://k6.io/docs/getting-started/installation/).

## Run

```bash
# Start server + infrastructure
docker compose up -d
cargo run --release --bin lumiere-server

# Message throughput (1000 msg/sec for 30s)
k6 run -e BASE_URL=http://localhost:8080 scenarios/message_throughput.js

# Message history (concurrent pagination)
k6 run -e BASE_URL=http://localhost:8080 scenarios/message_history.js

# WebSocket connections (ramp to 1000)
k6 run -e BASE_URL=http://localhost:8080 scenarios/websocket_connections.js

# Mixed realistic workload
k6 run -e BASE_URL=http://localhost:8080 scenarios/mixed_workload.js

# Connection storm (1000 connections in 10s)
k6 run -e BASE_URL=http://localhost:8080 scenarios/connection_storm.js
```

## Scenarios

| Scenario | Description |
|----------|-------------|
| `message_throughput` | Sustained 1000 msg/sec send rate for 30 seconds |
| `message_history` | Concurrent message history loading with cursor pagination |
| `websocket_connections` | Ramp to 1000 concurrent WebSocket connections with heartbeat |
| `mixed_workload` | Realistic usage: 70% reads, 20% sends, 5% browsing, 5% profile |
| `connection_storm` | HTTP + WebSocket burst: 1000 connections in 10 seconds |

## Performance Targets

| Metric | Target |
|--------|--------|
| Message send p99 | < 50ms |
| Message history p99 | < 20ms |
| WS connections/instance | > 10,000 |
| Message throughput | > 1,000 msg/sec |
| Mixed workload p95 | < 100ms |
