# Sprint 03 — Authentication

**Status:** Not Started
**Dependencies:** Sprint 02
**Crates:** lumiere-auth, lumiere-server, lumiere-db

## Goal

Implement full authentication system: registration, login, JWT access/refresh tokens, password hashing with Argon2id, session management in Redis, and auth middleware for Axum.

## Tasks

### 3.1 — Password Hashing

Use Argon2id (winner of Password Hashing Competition, recommended by OWASP):

```rust
// crates/lumiere-auth/src/password.rs
use argon2::{Argon2, PasswordHasher, PasswordVerifier};

pub fn hash_password(password: &str) -> Result<String> { ... }
pub fn verify_password(password: &str, hash: &str) -> Result<bool> { ... }
```

Parameters: Argon2id, m=19456 (19 MiB), t=2, p=1 (OWASP minimum recommendation).

### 3.2 — JWT Token System

Dual token approach:
- **Access token**: Short-lived (15 min), carries user claims, sent in `Authorization: Bearer` header
- **Refresh token**: Long-lived (7 days), stored in Redis, used to get new access tokens

```rust
// crates/lumiere-auth/src/jwt.rs

pub struct Claims {
    pub sub: String,       // user_id as string
    pub exp: u64,          // expiration timestamp
    pub iat: u64,          // issued at
    pub jti: String,       // unique token ID (for revocation)
    pub token_type: TokenType, // Access or Refresh
}

pub enum TokenType {
    Access,
    Refresh,
}

pub fn create_access_token(user_id: Snowflake, secret: &str) -> Result<String> { ... }
pub fn create_refresh_token(user_id: Snowflake, secret: &str) -> Result<String> { ... }
pub fn verify_token(token: &str, secret: &str) -> Result<Claims> { ... }
```

Use `jsonwebtoken` crate with HS256 algorithm.

### 3.3 — Session Management (Redis)

```
Key pattern:
  session:{user_id}:{jti}  → { device_info, ip, created_at, last_active }
  refresh:{jti}             → { user_id, expires_at }

TTL: matches token expiry
```

Operations:
- Create session on login
- Delete session on logout
- Delete all sessions on password change
- List active sessions for a user
- Update `last_active` on token refresh

### 3.4 — API Endpoints

```
POST /api/v1/auth/register
    Body: { username, email, password }
    Response: { user, access_token, refresh_token }
    Validation:
        - username: 2-32 chars, alphanumeric + underscore + dot
        - email: valid format, unique
        - password: 8-128 chars, at least 1 uppercase, 1 lowercase, 1 digit

POST /api/v1/auth/login
    Body: { email, password }
    Response: { user, access_token, refresh_token }

POST /api/v1/auth/refresh
    Body: { refresh_token }
    Response: { access_token, refresh_token }
    Note: Old refresh token is revoked (rotation)

POST /api/v1/auth/logout
    Headers: Authorization: Bearer <access_token>
    Response: 204 No Content
    Action: Revoke current refresh token, delete session

POST /api/v1/auth/logout-all
    Headers: Authorization: Bearer <access_token>
    Response: 204 No Content
    Action: Revoke ALL refresh tokens for user, delete all sessions
```

### 3.5 — Auth Middleware

Axum extractor that validates the access token and injects the authenticated user:

```rust
// crates/lumiere-auth/src/middleware.rs

pub struct AuthUser {
    pub id: Snowflake,
    pub username: String,
    pub flags: u64,
}

// Usage in handlers:
async fn get_me(auth: AuthUser) -> impl IntoResponse {
    // auth.id is the authenticated user's ID
}
```

The middleware:
1. Extracts `Authorization: Bearer <token>` header
2. Verifies JWT signature and expiration
3. Checks token is not revoked (Redis lookup)
4. Loads minimal user data
5. Returns `401 Unauthorized` if any step fails

### 3.6 — Optional Auth Middleware

Some endpoints work differently for authenticated vs anonymous users (e.g., invite preview). Create `MaybeAuthUser` extractor:

```rust
pub struct MaybeAuthUser(pub Option<AuthUser>);
```

### 3.7 — Input Validation

Use `validator` crate for request body validation:

```rust
#[derive(Deserialize, Validate)]
pub struct RegisterRequest {
    #[validate(length(min = 2, max = 32), regex(path = "USERNAME_REGEX"))]
    pub username: String,
    #[validate(email)]
    pub email: String,
    #[validate(length(min = 8, max = 128))]
    pub password: String,
}
```

Return structured validation errors:

```json
{
    "error": {
        "code": "VALIDATION_ERROR",
        "message": "Invalid input",
        "fields": {
            "username": ["must be between 2 and 32 characters"],
            "email": ["is not a valid email address"]
        }
    }
}
```

## Acceptance Criteria

- [ ] Password hashing with Argon2id works correctly
- [ ] JWT access tokens expire after 15 minutes
- [ ] JWT refresh tokens expire after 7 days
- [ ] Token refresh rotates refresh token (old one becomes invalid)
- [ ] Logout revokes the session
- [ ] Logout-all revokes all sessions for the user
- [ ] Auth middleware rejects expired/invalid/revoked tokens with 401
- [ ] Registration validates username, email, password
- [ ] Duplicate email/username returns 409 Conflict
- [ ] Integration tests: full register → login → refresh → logout flow
- [ ] Integration tests: concurrent token refresh (only one should succeed)
