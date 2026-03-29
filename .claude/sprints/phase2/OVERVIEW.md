# Phase 2 — Production Readiness Sprints

Phase 1 (Sprint 01-23) built the full feature set. Phase 2 makes it production-ready.

## Sprint Map

```
P2-01: Integration Testing & Smoke Tests
  ↓
P2-02: ScyllaDB Prepared Statements
  ↓
P2-03: Rate Limiting Middleware ←─── (Sprint 17 completion)
  ↓
P2-04: JetStream Consumers (Search Indexer, Push Worker)
  ↓
P2-05: Push Notifications — Real APNs & FCM
  ↓
P2-06: Gateway Event Replay (Resume)
  ↓
P2-07: Media Streaming (Upload/Download)
  ↓
P2-08: Security Hardening & Input Sanitization
  ↓
P2-09: End-to-End Testing & Load Testing
  ↓
P2-10: E2E Encryption (MLS Protocol) ←─── (Sprint 20 completion)
```

## Dependency Graph

```
P2-01 (Integration Tests)
  ├──→ P2-02 (Prepared Statements)
  ├──→ P2-03 (Rate Limiting)
  └──→ P2-04 (JetStream Consumers)
           ├──→ P2-05 (Push — needs consumer for delivery)
           └──→ P2-06 (Gateway Resume — needs event buffer)
P2-07 (Media Streaming) — independent
P2-08 (Security) — after P2-03
P2-09 (E2E Testing) — after P2-01 through P2-08
P2-10 (E2E Encryption) — independent, can start anytime
```

## Priority Order

| Priority | Sprint | Why |
|----------|--------|-----|
| MUST | P2-01 | Can't verify anything works without testing against real infra |
| MUST | P2-02 | ScyllaDB will choke without prepared statements under any load |
| MUST | P2-03 | No rate limiting = instant DDoS/abuse vulnerability |
| MUST | P2-04 | Messages aren't searchable and push doesn't fire without consumers |
| SHOULD | P2-05 | Mobile app needs push to be usable |
| SHOULD | P2-06 | Users lose messages on reconnect without replay |
| SHOULD | P2-07 | Large files crash the server (OOM) |
| SHOULD | P2-08 | Security headers, input sanitization, IP blocking |
| MUST | P2-09 | Verify everything works end-to-end before release |
| NICE | P2-10 | E2E encryption is a differentiator but not MVP |
