# P2-09 — End-to-End Testing & Load Testing

**Status:** Not Started
**Dependencies:** P2-01 through P2-08
**Crates:** All

## Goal

Full end-to-end verification and load testing before production release. This is the final gate.

## Tasks

### 9.1 — E2E Scenario Tests

Complete user journeys, not just individual endpoints:

```
Scenario: New user onboarding
  1. Register → 2. Login → 3. Create server → 4. Invite friend
  → 5. Friend joins → 6. Send message → 7. Friend receives via WS

Scenario: Real-time messaging
  1. Two users connect WS → 2. Both in same channel
  → 3. User A sends message → 4. User B receives MESSAGE_CREATE
  → 5. User A edits → 6. User B receives MESSAGE_UPDATE

Scenario: Permission enforcement
  1. Create server → 2. Create role with limited perms
  → 3. Assign role → 4. Verify denied actions return 403

Scenario: Voice channel
  1. User joins voice channel → 2. Gets LiveKit token
  → 3. Voice state broadcast to other members

Scenario: DM flow
  1. Send friend request → 2. Accept → 3. Create DM
  → 4. Send message → 5. Receive push notification
```

### 9.2 — Load Testing with k6

Install k6 and write scenarios:

```
load-test/
├── scenarios/
│   ├── websocket_connections.js   # 10K concurrent WS connections
│   ├── message_throughput.js      # 1000 msg/sec to single channel
│   ├── message_history.js         # Concurrent history loading
│   ├── connection_storm.js        # 1000 connects in 10 seconds
│   └── mixed_workload.js          # Realistic usage pattern
├── helpers/
│   └── auth.js                    # Login helper
└── k6.config.js
```

### 9.3 — Performance Targets

| Metric | Target |
|--------|--------|
| Message send p99 latency | < 50ms |
| Message history p99 latency | < 20ms |
| WebSocket connections per instance | > 10,000 |
| Message throughput (single channel) | > 1,000 msg/sec |
| API response time (general) p99 | < 100ms |

### 9.4 — CI Pipeline

GitHub Actions workflow:
```yaml
on: [push, pull_request]
jobs:
  test:
    - cargo fmt --check
    - cargo clippy -- -D warnings
    - cargo test
    - docker compose -f docker-compose.test.yml up -d
    - cargo test --test integration
    - docker compose -f docker-compose.test.yml down
```

## Acceptance Criteria

- [ ] All E2E scenarios pass
- [ ] Load test results meet performance targets
- [ ] CI pipeline runs on every PR
- [ ] No regressions from Phase 1 tests
