# P2-03 — Rate Limiting Middleware

**Status:** Not Started
**Dependencies:** P2-01
**Crates:** lumiere-server

## Goal

Implement Sprint 17's rate limiting as actual Axum middleware. Without this, the server is trivially DDoSable.

## Tasks

### 3.1 — Token Bucket in Redis (Lua Script)

Atomic Lua script for token bucket:
```lua
local key = KEYS[1]
local rate = tonumber(ARGV[1])      -- tokens per window
local window = tonumber(ARGV[2])    -- window in seconds
local now = tonumber(ARGV[3])       -- current timestamp

local bucket = redis.call('HMGET', key, 'tokens', 'last_refill')
local tokens = tonumber(bucket[1]) or rate
local last = tonumber(bucket[2]) or now

-- Refill tokens
local elapsed = now - last
local refill = math.floor(elapsed * rate / window)
tokens = math.min(rate, tokens + refill)

if tokens < 1 then
    local ttl = redis.call('PTTL', key)
    return {0, ttl}  -- rejected, retry_after_ms
end

tokens = tokens - 1
redis.call('HMSET', key, 'tokens', tokens, 'last_refill', now)
redis.call('EXPIRE', key, window * 2)
return {1, tokens}  -- allowed, remaining
```

### 3.2 — Axum Rate Limit Layer

Create `RateLimitLayer` middleware:
- Extracts user_id from JWT (or IP for unauthenticated endpoints)
- Calls Redis Lua script
- On rejection: returns 429 with `Retry-After` header
- On success: adds rate limit headers to response

```
X-RateLimit-Limit: 50
X-RateLimit-Remaining: 47
X-RateLimit-Reset: 1617235200
Retry-After: 3  (only on 429)
```

### 3.3 — Per-Route Rate Limits

| Route | Limit | Key |
|-------|-------|-----|
| POST /auth/register | 5/hour | IP |
| POST /auth/login | 10/10min | IP |
| POST /auth/refresh | 30/min | user_id |
| POST /channels/:id/messages | 5/5sec | user_id+channel_id |
| PUT /reactions/:emoji/@me | 1/0.25sec | user_id |
| POST /channels/:id/typing | 1/10sec | user_id+channel_id |
| PATCH /users/@me | 5/min | user_id |
| POST /servers | 10/hour | user_id |
| Global (all routes) | 50/sec | user_id |

### 3.4 — IP-Based Abuse Prevention

- Track failed login attempts per IP
- 10 failures in 10 minutes → block IP for 30 minutes
- Redis key: `ip_block:{ip}` with TTL
- Check in login handler before password verification

### 3.5 — Request Body Size Limits

Apply `tower_http::limit::RequestBodyLimitLayer` per route group:
- Message send: 8KB (content + embeds JSON)
- File upload: 50MB (attachment)
- General: 1MB default

## Acceptance Criteria

- [ ] Token bucket rate limiter works atomically in Redis
- [ ] Rate limit headers on every response
- [ ] 429 response with Retry-After when exceeded
- [ ] Per-route limits enforced
- [ ] IP blocking after failed logins
- [ ] Request body size limits prevent memory abuse
- [ ] Integration test: verify rate limits trigger correctly
