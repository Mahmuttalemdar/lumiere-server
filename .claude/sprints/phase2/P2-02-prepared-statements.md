# P2-02 — ScyllaDB Prepared Statements

**Status:** Not Started
**Dependencies:** P2-01
**Crates:** lumiere-server, lumiere-db

## Goal

Replace all `query_unpaged()` string calls with prepared statements. Currently every ScyllaDB query is parsed on every execution — this is the single biggest performance bottleneck.

## Tasks

### 2.1 — Prepared Statement Registry

Create a `PreparedStatements` struct in `lumiere-db` that holds all prepared statements:

```rust
pub struct PreparedStatements {
    pub insert_message: PreparedStatement,
    pub get_messages_before: PreparedStatement,
    pub get_messages_after: PreparedStatement,
    pub get_messages_default: PreparedStatement,
    pub get_message_by_id: PreparedStatement,
    pub update_message_content: PreparedStatement,
    pub soft_delete_message: PreparedStatement,
    pub insert_reaction: PreparedStatement,
    pub delete_reaction: PreparedStatement,
    pub get_reactions: PreparedStatement,
    pub insert_pin: PreparedStatement,
    pub delete_pin: PreparedStatement,
    pub get_pins: PreparedStatement,
    pub insert_read_state: PreparedStatement,
    pub get_read_states: PreparedStatement,
    // ... all ScyllaDB queries
}
```

Prepare all statements at startup in `Database::connect()`.

### 2.2 — Replace All query_unpaged Calls

For each CQL query in the codebase:
1. Add the prepared statement to the registry
2. Replace `state.db.scylla.query_unpaged(sql_string, params)` with `state.db.scylla.execute(&state.db.prepared.xxx, params)`

Files to update:
- `routes/messages.rs` — ~15 queries (insert, select, update, delete)
- `routes/reactions.rs` — ~6 queries
- `routes/typing.rs` — ~3 queries
- `gateway/handler.rs` — if any ScyllaDB calls exist

### 2.3 — Handle LIMIT as Bind Parameter

ScyllaDB CQL supports `LIMIT ?` as a bind parameter. Replace:
```rust
format!("SELECT ... LIMIT {}", remaining)  // string interpolation ❌
```
with:
```rust
state.db.scylla.execute(&prepared, (channel_id, bucket, remaining as i32))  // bind ✓
```

### 2.4 — Add PreparedStatements to AppState

- Add `prepared: PreparedStatements` to the `Database` struct
- Initialize during `Database::connect()`
- Accessible via `state.db.prepared.xxx`

## Acceptance Criteria

- [ ] Zero `query_unpaged()` calls with literal SQL strings remain (except migrations)
- [ ] All CQL queries use prepared statements via `execute()`
- [ ] LIMIT uses bind parameters, not string interpolation
- [ ] Statements prepared once at startup, reused for all requests
- [ ] Existing tests still pass
- [ ] Benchmark: message send latency reduced by >30%
