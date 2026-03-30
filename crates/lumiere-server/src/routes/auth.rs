use axum::{
    extract::{FromRequest, State},
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use lumiere_auth::{jwt, middleware::AuthUser, password, session::SessionManager};
use lumiere_models::error::{AppError, FieldError};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use validator::Validate;

use crate::middleware::rate_limit;
use crate::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/register", post(register))
        .route("/login", post(login))
        .route("/refresh", post(refresh))
        .route("/logout", post(logout))
        .route("/logout-all", post(logout_all))
}

// ─── Request/Response types ─────────────────────────────────────

#[derive(Debug, Deserialize, Validate)]
pub struct RegisterRequest {
    #[validate(length(min = 2, max = 32))]
    pub username: String,
    #[validate(email)]
    pub email: String,
    #[validate(length(min = 8, max = 128))]
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Debug, Serialize)]
pub struct AuthResponse {
    pub user: UserResponse,
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Debug, Serialize)]
pub struct UserResponse {
    pub id: lumiere_models::snowflake::Snowflake,
    pub username: String,
    pub email: String,
    pub avatar: Option<String>,
    pub bio: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

// ─── Handlers ───────────────────────────────────────────────────

async fn register(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RegisterRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Validate input
    if let Err(errors) = body.validate() {
        let field_errors: Vec<FieldError> = errors
            .field_errors()
            .into_iter()
            .flat_map(|(field, errs)| {
                errs.iter().map(move |e| FieldError {
                    field: field.to_string(),
                    message: e
                        .message
                        .as_ref()
                        .map(|m| m.to_string())
                        .unwrap_or_else(|| format!("Invalid {}", field)),
                })
            })
            .collect();
        return Err(AppError::Validation(field_errors));
    }

    // Normalize
    let email = body.email.to_lowercase().trim().to_string();
    let username = body.username.trim().to_string();

    // Check uniqueness
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM users WHERE email = $1 OR username = $2)",
    )
    .bind(&email)
    .bind(&username)
    .fetch_one(&state.db.pg)
    .await?;

    if exists {
        return Err(AppError::AlreadyExists(
            "Email or username already taken".into(),
        ));
    }

    // Hash password
    let password_hash = password::hash_password(&body.password).map_err(AppError::Internal)?;

    // Generate ID
    let user_id = state.snowflake.next_id();

    // Insert user
    sqlx::query("INSERT INTO users (id, username, email, password_hash) VALUES ($1, $2, $3, $4)")
        .bind(user_id)
        .bind(&username)
        .bind(&email)
        .bind(&password_hash)
        .execute(&state.db.pg)
        .await?;

    // Create default settings
    sqlx::query("INSERT INTO user_settings (user_id) VALUES ($1)")
        .bind(user_id)
        .execute(&state.db.pg)
        .await?;

    // Generate tokens
    let (access_token, _) = jwt::create_access_token(
        user_id,
        &state.config.auth.jwt_secret,
        state.config.auth.access_token_ttl,
    )
    .map_err(AppError::Internal)?;

    let (refresh_token, refresh_jti) = jwt::create_refresh_token(
        user_id,
        &state.config.auth.jwt_secret,
        state.config.auth.refresh_token_ttl,
    )
    .map_err(AppError::Internal)?;

    // Store refresh session
    let session_mgr = SessionManager::new(state.redis.clone());
    session_mgr
        .create_session(
            &user_id.to_string(),
            &refresh_jti,
            state.config.auth.refresh_token_ttl,
            None,
            None,
        )
        .await
        .map_err(AppError::Internal)?;

    let now = chrono::Utc::now();
    Ok((
        StatusCode::CREATED,
        Json(AuthResponse {
            user: UserResponse {
                id: user_id,
                username,
                email,
                avatar: None,
                bio: None,
                created_at: now,
            },
            access_token,
            refresh_token,
        }),
    ))
}

async fn login(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<impl IntoResponse, AppError> {
    // Extract client IP before consuming the request body
    let client_ip = rate_limit::extract_client_ip(&req);

    // Check if IP is blocked due to repeated login failures
    let mut redis = state.redis.clone();
    if rate_limit::is_login_blocked(&mut redis, &client_ip).await? {
        return Err(AppError::RateLimited { retry_after: 1800 });
    }

    // Parse request body
    let body: LoginRequest = axum::Json::from_request(req, &state)
        .await
        .map_err(|_| AppError::BadRequest("Invalid request body".into()))?
        .0;

    let email = body.email.to_lowercase().trim().to_string();

    // Find user
    let row = sqlx::query_as::<_, (i64, String, String, Option<String>, Option<String>, chrono::DateTime<chrono::Utc>)>(
        "SELECT id, username, password_hash, avatar, bio, created_at FROM users WHERE email = $1 AND deleted_at IS NULL",
    )
    .bind(&email)
    .fetch_optional(&state.db.pg)
    .await?;

    let (user_id_raw, username, password_hash, avatar, bio, created_at) = match row {
        Some(r) => r,
        None => {
            // Record failure even for non-existent users to prevent enumeration
            let mut redis = state.redis.clone();
            let _ = rate_limit::record_login_failure(&mut redis, &client_ip).await;
            return Err(AppError::Unauthorized("Invalid email or password".into()));
        }
    };

    let user_id = lumiere_models::snowflake::Snowflake::from(user_id_raw);

    // Verify password
    let valid =
        password::verify_password(&body.password, &password_hash).map_err(AppError::Internal)?;

    if !valid {
        let mut redis = state.redis.clone();
        let _ = rate_limit::record_login_failure(&mut redis, &client_ip).await;
        return Err(AppError::Unauthorized("Invalid email or password".into()));
    }

    // Successful login — clear any accumulated failures
    let mut redis = state.redis.clone();
    let _ = rate_limit::clear_login_failures(&mut redis, &client_ip).await;

    // Generate tokens
    let (access_token, _) = jwt::create_access_token(
        user_id,
        &state.config.auth.jwt_secret,
        state.config.auth.access_token_ttl,
    )
    .map_err(AppError::Internal)?;

    let (refresh_token, refresh_jti) = jwt::create_refresh_token(
        user_id,
        &state.config.auth.jwt_secret,
        state.config.auth.refresh_token_ttl,
    )
    .map_err(AppError::Internal)?;

    // Store refresh session
    let session_mgr = SessionManager::new(state.redis.clone());
    session_mgr
        .create_session(
            &user_id.to_string(),
            &refresh_jti,
            state.config.auth.refresh_token_ttl,
            None,
            None,
        )
        .await
        .map_err(AppError::Internal)?;

    Ok(Json(AuthResponse {
        user: UserResponse {
            id: user_id,
            username,
            email,
            avatar,
            bio,
            created_at,
        },
        access_token,
        refresh_token,
    }))
}

async fn refresh(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RefreshRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Verify refresh token
    let claims = jwt::verify_token(&body.refresh_token, &state.config.auth.jwt_secret)
        .map_err(|_| AppError::Unauthorized("Invalid refresh token".into()))?;

    if claims.token_type != jwt::TokenType::Refresh {
        return Err(AppError::Unauthorized("Not a refresh token".into()));
    }

    let session_mgr = SessionManager::new(state.redis.clone());

    // Check session exists (not revoked)
    let session = session_mgr
        .get_session(&claims.jti)
        .await
        .map_err(AppError::Internal)?
        .ok_or_else(|| AppError::Unauthorized("Session revoked".into()))?;

    let user_id: lumiere_models::snowflake::Snowflake = session
        .user_id
        .parse()
        .map_err(|_| AppError::Internal(anyhow::anyhow!("Invalid user_id in session")))?;

    // Revoke old refresh token (rotation)
    session_mgr
        .revoke_session(&claims.jti, &session.user_id)
        .await
        .map_err(AppError::Internal)?;

    // Issue new tokens
    let (access_token, _) = jwt::create_access_token(
        user_id,
        &state.config.auth.jwt_secret,
        state.config.auth.access_token_ttl,
    )
    .map_err(AppError::Internal)?;

    let (refresh_token, refresh_jti) = jwt::create_refresh_token(
        user_id,
        &state.config.auth.jwt_secret,
        state.config.auth.refresh_token_ttl,
    )
    .map_err(AppError::Internal)?;

    // Store new session
    session_mgr
        .create_session(
            &session.user_id,
            &refresh_jti,
            state.config.auth.refresh_token_ttl,
            session.device_info.as_deref(),
            session.ip.as_deref(),
        )
        .await
        .map_err(AppError::Internal)?;

    Ok(Json(serde_json::json!({
        "access_token": access_token,
        "refresh_token": refresh_token,
    })))
}

async fn logout(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    // Revoke all sessions for this user on logout
    // A more granular approach would accept a refresh_token in the body
    let session_mgr = SessionManager::new(state.redis.clone());
    session_mgr
        .revoke_all_sessions(&auth.id.to_string())
        .await
        .map_err(AppError::Internal)?;

    Ok(StatusCode::NO_CONTENT)
}

async fn logout_all(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    let session_mgr = SessionManager::new(state.redis.clone());
    session_mgr
        .revoke_all_sessions(&auth.id.to_string())
        .await
        .map_err(AppError::Internal)?;

    Ok(StatusCode::NO_CONTENT)
}
