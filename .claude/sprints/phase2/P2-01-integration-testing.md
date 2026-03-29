# P2-01 — Integration Testing & Smoke Tests

**Status:** Not Started
**Dependencies:** None (first sprint)
**Crates:** lumiere-server (tests), all crates

## Goal

Verify the entire system actually works with real infrastructure. Currently 58 unit tests pass but ZERO tests hit a real database, Redis, NATS, or any external service.

## Tasks

### 1.1 — Test Infrastructure Setup

- Create `tests/` directory in workspace root for integration tests
- Add `docker-compose.test.yml` with isolated test instances (different ports)
- Create test helper crate or module with:
  - `TestApp` struct: spins up the full Axum app with real connections
  - `TestClient`: HTTP client wrapper for making API calls
  - Database cleanup between tests (truncate all tables)
  - Snowflake generator with test-specific machine_id

### 1.2 — Auth Flow Integration Tests

```
test_register_creates_user_and_returns_tokens
test_register_duplicate_email_fails
test_register_duplicate_username_fails
test_login_with_correct_credentials
test_login_with_wrong_password
test_refresh_token_rotation
test_logout_revokes_session
test_expired_access_token_rejected
```

### 1.3 — User System Integration Tests

```
test_get_me_returns_full_user
test_get_other_user_returns_public_only (no email)
test_update_profile_username
test_username_rate_limit (3rd change in hour rejected)
test_friend_request_flow (send → accept → friends)
test_block_prevents_friend_request
test_block_prevents_dm
test_delete_account_soft_deletes
test_delete_account_blocked_if_owns_server
```

### 1.4 — Server & Channel Integration Tests

```
test_create_server_creates_defaults (channels, @everyone role)
test_create_server_limit_100
test_invite_create_and_join
test_invite_max_uses_enforced
test_invite_expiry_enforced
test_banned_user_cannot_rejoin
test_kick_member
test_role_hierarchy_prevents_kicking_admin
test_channel_create_and_list
test_channel_max_500_per_server
test_permission_override_denies_access
```

### 1.5 — Messaging Integration Tests

```
test_send_message_persists_in_scylladb
test_get_messages_pagination_before
test_get_messages_pagination_after
test_edit_own_message
test_cannot_edit_others_message
test_delete_message_soft_deletes
test_pin_message_and_list_pins
test_slowmode_enforced
test_timed_out_member_cannot_send
```

### 1.6 — WebSocket Gateway Smoke Test

```
test_ws_connect_and_identify
test_ws_heartbeat_keeps_alive
test_ws_heartbeat_timeout_closes
test_ws_receive_message_create_event
test_ws_resume_after_disconnect
```

## Acceptance Criteria

- [ ] `docker-compose.test.yml` works with `docker compose -f docker-compose.test.yml up -d`
- [ ] All integration tests pass with `cargo test --test integration`
- [ ] Tests are isolated — can run in parallel without interference
- [ ] CI-ready: tests can run in GitHub Actions with service containers
- [ ] At least 40 integration tests covering critical paths
