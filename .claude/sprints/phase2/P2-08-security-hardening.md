# P2-08 — Security Hardening & Input Sanitization

**Status:** Not Started
**Dependencies:** P2-03 (rate limiting)
**Crates:** lumiere-server

## Goal

Add security headers, input sanitization, and abuse prevention beyond rate limiting.

## Tasks

### 8.1 — Security Headers Middleware

Add via `tower_http::set_header`:
```
X-Content-Type-Options: nosniff
X-Frame-Options: DENY
X-XSS-Protection: 0
Referrer-Policy: strict-origin-when-cross-origin
Content-Security-Policy: default-src 'none'
Strict-Transport-Security: max-age=31536000; includeSubDomains (production only)
Permissions-Policy: camera=(), microphone=(), geolocation=()
```

### 8.2 — Input Sanitization

Create sanitization functions applied to all user input:
- Strip null bytes (`\0`) from all string inputs
- Strip control characters (U+0000-U+001F except \n \r \t)
- Unicode NFC normalization for usernames
- Trim excessive whitespace (collapse multiple spaces)
- Max depth validation for JSON payloads (embeds, webhook body)
- Apply to: message content, usernames, server names, channel names, bios, topics

### 8.3 — Content-Disposition on File Serving

When serving files through the server (not presigned URLs):
- Always set `Content-Disposition: attachment` for non-image types
- For images: `Content-Disposition: inline` with `X-Content-Type-Options: nosniff`
- Never serve HTML or SVG inline

### 8.4 — CSRF Protection

- Verify `Origin` header matches allowed origins on state-changing requests
- SameSite=Strict on any cookies (if used)
- Double-submit token pattern for web clients

### 8.5 — Audit Logging Enhancement

Extend audit log to capture:
- All permission changes (role create/update/delete, override changes)
- All moderation actions (ban, kick, timeout, warning)
- Server setting changes
- Channel create/delete
- Include before/after values in `changes` JSONB column

## Acceptance Criteria

- [ ] All security headers present on every response
- [ ] Null bytes and control characters stripped from input
- [ ] Unicode normalized for usernames
- [ ] File downloads have correct Content-Disposition
- [ ] Audit log captures all moderation and permission changes
