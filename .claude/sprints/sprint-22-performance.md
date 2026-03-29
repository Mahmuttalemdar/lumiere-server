# Sprint 22 — Performance & Load Testing

**Status:** Not Started
**Dependencies:** Sprint 16
**Crates:** All crates

## Goal

Comprehensive load testing, benchmarking, profiling, and optimization. Verify the system meets performance targets under realistic load.

## Tasks

### 22.1 — Performance Targets

| Metric | Target |
|--------|--------|
| Message send → WebSocket delivery | < 50ms p99 |
| Message history query (50 msgs) | < 20ms p99 |
| REST API response time | < 100ms p99 |
| WebSocket connections per instance | 100,000+ |
| Messages per second (single channel) | 1,000+ |
| Messages per second (global) | 50,000+ |
| Search latency | < 50ms p99 |
| Voice join latency | < 200ms p99 |
| Memory per WebSocket connection | < 15 KB |
| Cold start time | < 5 seconds |

### 22.2 — Load Testing Framework

Use `k6` for HTTP/WebSocket load testing + custom Rust benchmark harness:

```javascript
// k6/message_load_test.js
import ws from 'k6/ws';
import http from 'k6/http';

export const options = {
    scenarios: {
        websocket_connections: {
            executor: 'ramping-vus',
            startVUs: 0,
            stages: [
                { duration: '2m', target: 1000 },
                { duration: '5m', target: 5000 },
                { duration: '5m', target: 10000 },
                { duration: '2m', target: 0 },
            ],
        },
        message_send: {
            executor: 'constant-arrival-rate',
            rate: 1000,
            timeUnit: '1s',
            duration: '10m',
            preAllocatedVUs: 500,
        },
    },
};
```

### 22.3 — Benchmark Suite

Custom Rust benchmarks using `criterion`:

```rust
// benches/
// ├── snowflake_bench.rs      — ID generation throughput
// ├── permission_bench.rs     — Permission calculation speed
// ├── mention_parse_bench.rs  — Message mention parsing
// ├── bucket_bench.rs         — Bucket calculation
// ├── serialization_bench.rs  — Message JSON serialization
```

### 22.4 — Test Scenarios

**Scenario 1: Chat Storm**
- 10,000 connected users in one server
- 100 channels
- 1,000 messages/second distributed across channels
- Measure: message delivery latency, CPU, memory

**Scenario 2: Server Join Flood**
- 1,000 users joining a server in 1 minute
- Each triggers: member add, role assign, GUILD_CREATE with full server data
- Measure: event delivery time, database write throughput

**Scenario 3: Hot Channel**
- 50,000 users viewing one channel
- 100 messages/second in that channel
- Measure: request coalescing effectiveness, ScyllaDB query count

**Scenario 4: Connection Storm**
- 50,000 WebSocket connections in 5 minutes
- Each sends Identify, receives Ready
- Measure: connection accept rate, memory growth, NATS subscription count

**Scenario 5: Message History Scroll**
- 1,000 users scrolling through message history simultaneously
- Each paginating backwards through 1,000 messages
- Measure: ScyllaDB read latency, bucket traversal efficiency

**Scenario 6: Mixed Workload**
- Realistic mix: 60% message reads, 20% message sends, 10% presence, 10% other
- 10,000 concurrent users
- Measure: overall system throughput and latency distribution

### 22.5 — Profiling

**CPU Profiling:**
```bash
# Use flamegraph via cargo-flamegraph
cargo flamegraph --bin lumiere-server
```

Look for:
- Hot loops in WebSocket message dispatch
- JSON serialization overhead
- Permission calculation on every message
- NATS publish overhead

**Memory Profiling:**
```bash
# Use DHAT or jemalloc profiling
DHAT_LOG=dhat.log cargo run --features dhat-heap
```

Look for:
- Per-connection memory allocation
- String allocations in message processing
- NATS subscription memory
- Redis connection overhead

**Async Runtime Analysis:**
- Tokio task count and scheduling delays
- Task spawning patterns (too many small tasks?)
- Channel buffer sizes

### 22.6 — Optimization Targets

Based on profiling, likely optimizations:

**Serialization:**
- Use `simd-json` for JSON parsing
- Pre-serialize frequently sent events (READY payload)
- Zero-copy deserialization where possible (`serde_json::RawValue`)

**Memory:**
- Use `bytes::Bytes` for zero-copy message passing
- Reduce cloning in event fanout (Arc-wrap large payloads)
- Pool allocators for hot-path objects

**Database:**
- Prepared statement caching (ScyllaDB driver does this automatically)
- Batch reads when loading server data (channels, roles, members in parallel)
- Optimize cursor-based pagination queries

**WebSocket:**
- Frame-level compression (permessage-deflate)
- Batch event delivery (multiple events in one WebSocket frame)
- Reduce per-message allocation in broadcast

**NATS:**
- Use NATS headers for metadata instead of embedding in payload
- Batch publishes where possible
- Tune NATS buffer sizes

### 22.7 — Regression Testing

CI pipeline runs performance regression tests:
- Benchmark suite on every PR
- Compare against baseline
- Fail if p99 latency regresses > 10%
- Track memory usage per WebSocket connection

## Acceptance Criteria

- [ ] k6 load test suite covers all major scenarios
- [ ] Criterion benchmark suite for critical code paths
- [ ] All performance targets met (table above)
- [ ] Request coalescing reduces DB queries by > 90% in hot channel scenario
- [ ] Cache hit rate > 80% for active channels
- [ ] Memory per connection < 15 KB
- [ ] CPU flamegraph shows no unexpected hot spots
- [ ] No memory leaks under sustained load (24h soak test)
- [ ] CI runs benchmark suite and catches regressions
- [ ] Optimization document created with before/after numbers
