use axum::{
    extract::{ConnectInfo, Request, State},
    http::{HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use redis::AsyncCommands;
use std::net::SocketAddr;
use std::sync::{Arc, LazyLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::AppState;

// ─── Lua Token Bucket Script ───────────────────────────────────────────────

/// Redis Lua script implementing a token bucket rate limiter.
/// Uses Redis server time (milliseconds) to avoid client clock skew.
/// Returns: {allowed (0/1), retry_after_ms, remaining_tokens}
const RATE_LIMIT_SCRIPT: &str = r#"
local key = KEYS[1]
local rate = tonumber(ARGV[1])
local window_ms = tonumber(ARGV[2]) * 1000

local time = redis.call('TIME')
local now = tonumber(time[1]) * 1000 + math.floor(tonumber(time[2]) / 1000)

local data = redis.call('HMGET', key, 'tokens', 'last')
local tokens = tonumber(data[1])
local last = tonumber(data[2])

if tokens == nil then
    tokens = rate
    last = now
end

local elapsed = math.max(0, now - last)
local refill = elapsed * rate / window_ms
tokens = math.min(rate, tokens + refill)

if tokens < 1 then
    local wait = math.ceil((1 - tokens) * window_ms / rate)
    return {0, wait, math.floor(tokens)}
end

tokens = tokens - 1
redis.call('HMSET', key, 'tokens', tostring(tokens), 'last', tostring(now))
redis.call('EXPIRE', key, math.ceil(window_ms / 1000) * 2)
return {1, 0, math.floor(tokens)}
"#;

/// Lua script for atomic login failure tracking (INCR + EXPIRE without race).
const LOGIN_FAILURE_SCRIPT: &str = r#"
local count = redis.call('INCR', KEYS[1])
if count == 1 then redis.call('EXPIRE', KEYS[1], ARGV[1]) end
return count
"#;

static RATE_LIMIT_LUA: LazyLock<redis::Script> =
    LazyLock::new(|| redis::Script::new(RATE_LIMIT_SCRIPT));

static LOGIN_FAILURE_LUA: LazyLock<redis::Script> =
    LazyLock::new(|| redis::Script::new(LOGIN_FAILURE_SCRIPT));

// ─── Config ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct RateLimitConfig {
    pub key_prefix: String,
    pub max_requests: u32,
    pub window_seconds: u32,
}

/// Result from a rate limit check.
pub struct RateLimitResult {
    pub allowed: bool,
    pub remaining: u32,
    pub limit: u32,
    pub retry_after: Option<u32>,
}

// ─── Predefined Rate Limits ────────────────────────────────────────────────

/// 50 requests per second per user — global baseline.
pub fn global_limit() -> RateLimitConfig {
    RateLimitConfig {
        key_prefix: "rl:global".into(),
        max_requests: 120,
        window_seconds: 1,
    }
}

/// 5 registrations per hour per IP.
pub fn auth_register_limit() -> RateLimitConfig {
    RateLimitConfig {
        key_prefix: "rl:register".into(),
        max_requests: 5,
        window_seconds: 3600,
    }
}

/// 10 login attempts per 10 minutes per IP.
pub fn auth_login_limit() -> RateLimitConfig {
    RateLimitConfig {
        key_prefix: "rl:login".into(),
        max_requests: 10,
        window_seconds: 600,
    }
}

/// 5 messages per 5 seconds per user.
pub fn message_send_limit() -> RateLimitConfig {
    RateLimitConfig {
        key_prefix: "rl:msg".into(),
        max_requests: 5,
        window_seconds: 5,
    }
}

/// 4 reactions per second per user.
pub fn reaction_limit() -> RateLimitConfig {
    RateLimitConfig {
        key_prefix: "rl:react".into(),
        max_requests: 4,
        window_seconds: 1,
    }
}

// ─── Core Check ────────────────────────────────────────────────────────────

/// Execute the token bucket rate limit check against Redis.
/// Uses Redis server time internally (no client-side timestamp).
pub async fn check_rate_limit(
    redis: &mut redis::aio::ConnectionManager,
    config: &RateLimitConfig,
    identifier: &str,
) -> Result<RateLimitResult, lumiere_models::error::AppError> {
    let key = format!("{}:{}", config.key_prefix, identifier);

    let result: Vec<i64> = RATE_LIMIT_LUA
        .key(&key)
        .arg(config.max_requests)
        .arg(config.window_seconds)
        .invoke_async(redis)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Rate limit Redis script failed");
            lumiere_models::error::AppError::Internal(anyhow::anyhow!(
                "Rate limit service unavailable"
            ))
        })?;

    let allowed = result[0] == 1;
    let retry_after = result[1] as u32;
    let remaining = result[2] as u32;

    Ok(RateLimitResult {
        allowed,
        remaining,
        limit: config.max_requests,
        retry_after: if allowed { None } else { Some(retry_after) },
    })
}

// ─── IP-Based Login Blocking ───────────────────────────────────────────────

const LOGIN_FAIL_MAX: u32 = 10;
const LOGIN_BLOCK_SECONDS: u64 = 1800; // 30 minutes

/// Check if an IP is blocked due to repeated login failures.
pub async fn is_login_blocked(
    redis: &mut redis::aio::ConnectionManager,
    ip: &str,
) -> Result<bool, lumiere_models::error::AppError> {
    let block_key = format!("ip_block:{}", ip);
    let blocked: Option<String> = redis.get(&block_key).await.map_err(|e| {
        lumiere_models::error::AppError::Internal(anyhow::anyhow!("Redis error: {}", e))
    })?;
    Ok(blocked.is_some())
}

/// Record a failed login attempt. If the threshold is exceeded, block the IP.
/// Uses atomic Lua script to avoid INCR+EXPIRE race condition.
pub async fn record_login_failure(
    redis: &mut redis::aio::ConnectionManager,
    ip: &str,
) -> Result<(), lumiere_models::error::AppError> {
    let fail_key = format!("ip_fail:{}", ip);
    let count: u32 = LOGIN_FAILURE_LUA
        .key(&fail_key)
        .arg(LOGIN_BLOCK_SECONDS as i64)
        .invoke_async(redis)
        .await
        .map_err(|e| {
            lumiere_models::error::AppError::Internal(anyhow::anyhow!("Redis error: {}", e))
        })?;

    if count >= LOGIN_FAIL_MAX {
        let block_key = format!("ip_block:{}", ip);
        let _: Result<(), _> = redis::cmd("SET")
            .arg(&block_key)
            .arg("1")
            .arg("EX")
            .arg(LOGIN_BLOCK_SECONDS as i64)
            .query_async(redis)
            .await;
    }

    Ok(())
}

/// Clear failed login attempts on successful login.
pub async fn clear_login_failures(
    redis: &mut redis::aio::ConnectionManager,
    ip: &str,
) -> Result<(), lumiere_models::error::AppError> {
    let fail_key = format!("ip_fail:{}", ip);
    let _: Result<(), _> = redis::cmd("DEL")
        .arg(&fail_key)
        .query_async(redis)
        .await;
    Ok(())
}

// ─── Identifier Extraction ─────────────────────────────────────────────────

/// Extract the rate-limit identifier from the request.
/// Prefers user ID from Authorization header, falls back to client IP.
fn extract_identifier(req: &Request) -> String {
    // Try to get user ID from Authorization header (already-decoded JWT)
    if let Some(auth_header) = req.headers().get(axum::http::header::AUTHORIZATION) {
        if let Ok(value) = auth_header.to_str() {
            if let Some(token) = value.strip_prefix("Bearer ") {
                // Decode the JWT claims to extract user_id without full verification
                // (verification happens later in the auth middleware). We just need the sub claim.
                if let Some(user_id) = extract_user_id_from_token(token) {
                    return format!("user:{}", user_id);
                }
            }
        }
    }

    // Fall back to IP from common proxy headers, then ConnectInfo
    extract_client_ip(req)
}

/// Extract the client IP from the request.
/// Prefers the direct connection IP (ConnectInfo) to prevent spoofing via
/// X-Forwarded-For. Only falls back to proxy headers when ConnectInfo is
/// not available (e.g. behind a reverse proxy that strips it).
pub fn extract_client_ip(req: &Request) -> String {
    // Prefer direct connection IP over proxy headers to prevent spoofing
    if let Some(connect_info) = req.extensions().get::<ConnectInfo<SocketAddr>>() {
        return format!("ip:{}", connect_info.0.ip());
    }

    // Fallback to headers only if ConnectInfo not available
    if let Some(forwarded_for) = req.headers().get("x-forwarded-for") {
        if let Ok(value) = forwarded_for.to_str() {
            if let Some(first_ip) = value.split(',').next() {
                return format!("ip:{}", first_ip.trim());
            }
        }
    }

    if let Some(real_ip) = req.headers().get("x-real-ip") {
        if let Ok(value) = real_ip.to_str() {
            return format!("ip:{}", value.trim());
        }
    }

    "ip:unknown".to_string()
}

/// Lightweight JWT subject extraction (no signature verification).
fn extract_user_id_from_token(token: &str) -> Option<String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    // Decode the payload (second part)
    use base64::Engine;
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let payload_bytes = engine.decode(parts[1]).ok()?;
    let payload: serde_json::Value = serde_json::from_slice(&payload_bytes).ok()?;
    payload.get("sub").and_then(|v| v.as_str()).map(|s| s.to_string())
}

// ─── Axum Middleware ───────────────────────────────────────────────────────

/// Global rate limit middleware. Applied to all routes.
pub async fn global_rate_limit_middleware(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    let identifier = extract_identifier(&req);
    let config = global_limit();

    let mut redis = state.redis.clone();
    match check_rate_limit(&mut redis, &config, &identifier).await {
        Ok(result) => {
            if !result.allowed {
                return rate_limit_response(&result);
            }
            let response = next.run(req).await;
            add_rate_limit_headers(response, &result)
        }
        Err(_) => {
            // If Redis is unavailable, allow the request (fail-open)
            tracing::warn!("Rate limit check failed, allowing request (fail-open)");
            next.run(req).await
        }
    }
}

/// Create a per-route rate limit middleware function.
/// Returns an axum middleware function for the given config.
pub fn route_rate_limit(
    config: RateLimitConfig,
) -> impl Fn(
    State<Arc<AppState>>,
    Request,
    Next,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>>
       + Clone
       + Send {
    move |State(state): State<Arc<AppState>>, req: Request, next: Next| {
        let config = config.clone();
        Box::pin(async move {
            let identifier = extract_identifier(&req);
            let mut redis = state.redis.clone();
            match check_rate_limit(&mut redis, &config, &identifier).await {
                Ok(result) => {
                    if !result.allowed {
                        return rate_limit_response(&result);
                    }
                    let response = next.run(req).await;
                    add_rate_limit_headers(response, &result)
                }
                Err(_) => {
                    tracing::warn!("Route rate limit check failed, allowing request (fail-open)");
                    next.run(req).await
                }
            }
        })
    }
}

/// IP-only rate limit middleware for unauthenticated routes (register, login).
pub fn ip_rate_limit(
    config: RateLimitConfig,
) -> impl Fn(
    State<Arc<AppState>>,
    Request,
    Next,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>>
       + Clone
       + Send {
    move |State(state): State<Arc<AppState>>, req: Request, next: Next| {
        let config = config.clone();
        Box::pin(async move {
            let identifier = extract_client_ip(&req);
            let mut redis = state.redis.clone();
            match check_rate_limit(&mut redis, &config, &identifier).await {
                Ok(result) => {
                    if !result.allowed {
                        return rate_limit_response(&result);
                    }
                    let response = next.run(req).await;
                    add_rate_limit_headers(response, &result)
                }
                Err(_) => {
                    tracing::warn!("IP rate limit check failed, allowing request (fail-open)");
                    next.run(req).await
                }
            }
        })
    }
}

// ─── Response Helpers ──────────────────────────────────────────────────────

fn rate_limit_response(result: &RateLimitResult) -> Response {
    let retry_after = result.retry_after.unwrap_or(1);
    let reset = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + retry_after as u64;

    let body = serde_json::json!({
        "error": {
            "code": "RATE_LIMITED",
            "message": "Rate limited",
            "retry_after": retry_after
        }
    });

    let mut response = (StatusCode::TOO_MANY_REQUESTS, axum::Json(body)).into_response();
    let headers = response.headers_mut();
    headers.insert(
        "x-ratelimit-limit",
        HeaderValue::from_str(&result.limit.to_string()).unwrap(),
    );
    headers.insert(
        "x-ratelimit-remaining",
        HeaderValue::from_str("0").unwrap(),
    );
    headers.insert(
        "x-ratelimit-reset",
        HeaderValue::from_str(&reset.to_string()).unwrap(),
    );
    headers.insert(
        "retry-after",
        HeaderValue::from_str(&retry_after.to_string()).unwrap(),
    );
    response
}

fn add_rate_limit_headers(mut response: Response, result: &RateLimitResult) -> Response {
    let reset = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 1; // next bucket reset

    let headers = response.headers_mut();
    headers.insert(
        "x-ratelimit-limit",
        HeaderValue::from_str(&result.limit.to_string()).unwrap(),
    );
    headers.insert(
        "x-ratelimit-remaining",
        HeaderValue::from_str(&result.remaining.to_string()).unwrap(),
    );
    headers.insert(
        "x-ratelimit-reset",
        HeaderValue::from_str(&reset.to_string()).unwrap(),
    );
    response
}
