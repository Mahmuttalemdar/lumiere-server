# P2-06 — Gateway Event Replay (Resume)

**Status:** Not Started
**Dependencies:** P2-04 (JetStream for event buffering)
**Crates:** lumiere-gateway, lumiere-nats

## Goal

When a client disconnects and resumes, replay all missed events. Currently resume just re-subscribes to NATS — events during disconnection are lost forever.

## Tasks

### 6.1 — Event Buffer in Redis

Store dispatched events per session in a Redis list:

```
Key: gateway_events:{session_id}
Value: List of JSON-encoded GatewayMessage
TTL: 5 minutes (same as session)
Max length: 1000 events (cap with LTRIM)
```

On every `Dispatch` event sent to a client:
1. Push event to Redis list: `RPUSH gateway_events:{session_id} {json}`
2. Trim to max length: `LTRIM gateway_events:{session_id} -1000 -1`
3. Refresh TTL: `EXPIRE gateway_events:{session_id} 300`

### 6.2 — Replay on Resume

When client sends Resume with `sequence`:
1. Fetch session from Redis (existing logic)
2. Fetch event buffer: `LRANGE gateway_events:{session_id} 0 -1`
3. Filter events where `s > payload.sequence`
4. Send filtered events in order to the client
5. Send RESUMED event
6. Continue with live events

### 6.3 — Buffer Cleanup

- On normal disconnect: keep buffer (client may resume)
- After 5 min TTL: Redis auto-expires buffer
- On successful resume: clear old buffer, start new one
- On Identify (new session): no replay needed

### 6.4 — Memory Considerations

- 1000 events × ~500 bytes = ~500KB per session in Redis
- 10,000 concurrent sessions = ~5GB Redis memory
- Use Redis memory policy: `volatile-lru` to evict old sessions first
- Consider compression for large events (server member lists)

## Acceptance Criteria

- [ ] Events buffered in Redis during active session
- [ ] Resume replays missed events in correct order
- [ ] Sequence numbers are continuous after replay
- [ ] Buffer expires after 5 minutes of disconnection
- [ ] Buffer capped at 1000 events
- [ ] Integration test: connect → receive events → disconnect → resume → verify missed events received
