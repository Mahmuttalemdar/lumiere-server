use axum::{
    extract::Request,
    http::HeaderValue,
    middleware::Next,
    response::Response,
};

/// Middleware that adds security headers to every response.
pub async fn security_headers_middleware(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    let headers = response.headers_mut();

    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    // Disable XSS auditor — modern browsers don't need it and it can cause issues
    headers.insert("x-xss-protection", HeaderValue::from_static("0"));
    headers.insert(
        "referrer-policy",
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    // camera and microphone allowed for voice/video features
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static("geolocation=()"),
    );
    // HSTS — enforce HTTPS
    headers.insert(
        "strict-transport-security",
        HeaderValue::from_static("max-age=63072000; includeSubDomains; preload"),
    );
    // Content Security Policy — API server should not serve HTML pages,
    // so default-src 'none' blocks all content loading if someone navigates
    // to an API endpoint directly.
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static("default-src 'none'"),
    );
    // Prevent caching of API responses that may contain sensitive data
    headers.insert(
        "cache-control",
        HeaderValue::from_static("no-store"),
    );
    // Cross-Origin isolation headers
    headers.insert(
        "cross-origin-opener-policy",
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("cross-origin"),
    );

    response
}
