# Sprint 17 — Rate Limiting & Security

**Status:** Not Started
**Dependencies:** Sprint 08
**Crates:** lumiere-server, lumiere-gateway

## Goal

Comprehensive rate limiting (global, per-route, per-user, per-IP), input sanitization, CORS, security headers, and abuse prevention.

## Tasks

### 17.1 — Rate Limiting Strategy

Token bucket algorithm implemented in Redis:

```rust
// crates/lumiere-server/src/rate_limit.rs

pub struct RateLimiter {
    redis: RedisClient,
}

impl RateLimiter {
    /// Check and consume a rate limit token
    /// Returns Ok(remaining) or Err(retry_after_seconds)
    pub async fn check(
        &self,
        key: &str,
        limit: u32,
        window_seconds: u64,
    ) -> Result<u32, u64> {
        // Lua script for atomic check-and-increment
        let script = r#"
            local key = KEYS[1]
            local limit = tonumber(ARGV[1])
            local window = tonumber(ARGV[2])
            local current = tonumber(redis.call('GET', key) or '0')
            if current >= limit then
                local ttl = redis.call('TTL', key)
                return {0, ttl}
            end
            current = redis.call('INCR', key)
            if current == 1 then
                redis.call('EXPIRE', key, window)
            end
            return {limit - current, 0}
        "#;

        // Execute and parse result
    }
}
```

### 17.2 — Rate Limit Tiers

```
Global rate limits (per user):
    - 50 requests per second (all endpoints combined)

Per-route limits:
    POST /api/v1/auth/register          → 5 per hour per IP
    POST /api/v1/auth/login             → 10 per 10 min per IP
    POST /api/v1/auth/refresh           → 30 per hour per user
    POST /api/v1/channels/:id/messages  → 5 per 5 sec per user per channel
    PATCH /api/v1/users/@me             → 5 per minute per user
    POST /api/v1/channels/:id/typing    → 1 per 10 sec per user per channel
    POST /api/v1/servers                → 10 per day per user
    POST /api/v1/channels/:id/invites   → 5 per minute per user
    PUT .../reactions/:emoji/@me        → 1 per 0.25 sec per user
    DELETE .../messages (bulk)          → 1 per 30 sec per user per channel
    POST /api/v1/channels/:id/attachments → 10 per minute per user

WebSocket rate limits (per connection):
    - 120 gateway commands per 60 seconds
    - Identify: 1 per 5 seconds
    - Presence update: 5 per 60 seconds
```

### 17.3 — Rate Limit Headers

Return rate limit info in response headers:

```
X-RateLimit-Limit: 5
X-RateLimit-Remaining: 3
X-RateLimit-Reset: 1234567890    (Unix timestamp)
X-RateLimit-Bucket: channel_messages
Retry-After: 2                    (Only on 429 responses)
```

### 17.4 — Rate Limit Middleware

Axum layer that applies rate limiting before handlers:

```rust
pub struct RateLimitLayer {
    limiter: Arc<RateLimiter>,
    config: RateLimitConfig,
}

pub struct RateLimitConfig {
    pub key: RateLimitKey,
    pub limit: u32,
    pub window_seconds: u64,
}

pub enum RateLimitKey {
    Ip,
    UserId,
    UserIdAndRoute,
    UserIdAndChannel,
    IpAndRoute,
}
```

### 17.5 — Input Sanitization

All user input must be sanitized:

```rust
pub fn sanitize_message_content(content: &str) -> String {
    // 1. Trim whitespace
    // 2. Reject null bytes
    // 3. Limit to 4000 characters
    // 4. Reject control characters (except newline, tab)
    // 5. Normalize Unicode (NFC)
    // 6. Strip zero-width characters that could be used for invisible text
}

pub fn sanitize_username(username: &str) -> Result<String> {
    // 1. Trim
    // 2. Reject if < 2 or > 32 chars
    // 3. Only allow: alphanumeric, underscore, dot
    // 4. Reject if starts/ends with dot
    // 5. Reject consecutive dots
    // 6. Case-insensitive uniqueness check
}

pub fn sanitize_server_name(name: &str) -> Result<String> {
    // 1. Trim
    // 2. 1-100 chars
    // 3. Reject control characters
}
```

### 17.6 — CORS Configuration

```rust
let cors = CorsLayer::new()
    .allow_origin(config.allowed_origins.clone()) // In dev: Any, in prod: specific domains
    .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::PUT, Method::DELETE])
    .allow_headers([
        header::AUTHORIZATION,
        header::CONTENT_TYPE,
        header::ACCEPT,
        HeaderName::from_static("x-audit-log-reason"),
    ])
    .expose_headers([
        HeaderName::from_static("x-ratelimit-limit"),
        HeaderName::from_static("x-ratelimit-remaining"),
        HeaderName::from_static("x-ratelimit-reset"),
        HeaderName::from_static("retry-after"),
    ])
    .max_age(Duration::from_secs(86400));
```

### 17.7 — Security Headers

```rust
// Applied to all responses
pub fn security_headers() -> impl Layer {
    SetResponseHeaderLayer::if_not_present(
        HeaderName::from_static("x-content-type-options"), HeaderValue::from_static("nosniff"),
    )
    .layer(SetResponseHeaderLayer::if_not_present(
        HeaderName::from_static("x-frame-options"), HeaderValue::from_static("DENY"),
    ))
    .layer(SetResponseHeaderLayer::if_not_present(
        HeaderName::from_static("x-xss-protection"), HeaderValue::from_static("0"),
    ))
    .layer(SetResponseHeaderLayer::if_not_present(
        HeaderName::from_static("referrer-policy"), HeaderValue::from_static("no-referrer"),
    ))
    .layer(SetResponseHeaderLayer::if_not_present(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    ))
}
```

### 17.8 — IP-Based Abuse Prevention

- Track failed login attempts per IP
- After 10 failed attempts in 10 minutes: temporary IP block (30 min)
- Configurable IP allowlist/blocklist

```rust
pub struct IpBlocker {
    redis: RedisClient,
}

impl IpBlocker {
    pub async fn record_failure(&self, ip: &str) -> Result<()> { ... }
    pub async fn is_blocked(&self, ip: &str) -> bool { ... }
    pub async fn block(&self, ip: &str, duration: Duration) -> Result<()> { ... }
}
```

### 17.9 — Request Size Limits

```rust
// Axum body size limits
let app = Router::new()
    .layer(DefaultBodyLimit::max(25 * 1024 * 1024))  // 25 MB default
    .route("/api/v1/channels/:id/messages", post(send_message)
        .layer(DefaultBodyLimit::max(8 * 1024)));     // 8 KB for message body
```

### 17.10 — Audit Log Integration

Security-relevant actions logged to the audit_log table:
- Role changes
- Channel permission changes
- Member kicks/bans
- Server settings changes
- Webhook create/delete

Each action records: who, what, when, target, changes (before/after JSONB).

## Acceptance Criteria

- [ ] Global rate limit: 50 req/sec per user enforced
- [ ] Per-route rate limits work with correct windows
- [ ] Rate limit headers returned on every response
- [ ] 429 response with Retry-After on limit exceeded
- [ ] WebSocket rate limits enforced per connection
- [ ] Input sanitization rejects null bytes, control chars
- [ ] Username validation enforced
- [ ] CORS headers correct for development and production
- [ ] Security headers present on all responses
- [ ] IP blocking after repeated failed login attempts
- [ ] Request body size limits per route
- [ ] Audit log entries created for security-relevant actions
- [ ] Rate limiting uses atomic Redis operations (no race conditions)
- [ ] Integration test: send requests at rate limit boundary → verify 429
