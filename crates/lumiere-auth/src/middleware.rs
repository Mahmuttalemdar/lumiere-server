use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use lumiere_models::snowflake::Snowflake;
use serde_json::json;
use std::sync::Arc;

use crate::jwt::{self, TokenType};

/// Authenticated user extracted from the Authorization header
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: Snowflake,
    pub jti: String,
}

/// State required by the auth middleware
pub trait AuthState: Send + Sync + 'static {
    fn jwt_secret(&self) -> &str;
}

impl<S> FromRequestParts<Arc<S>> for AuthUser
where
    S: AuthState,
{
    type Rejection = AuthError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<S>,
    ) -> Result<Self, Self::Rejection> {
        let token = extract_bearer_token(parts)?;
        let claims = jwt::verify_token(&token, state.jwt_secret())
            .map_err(|_| AuthError::InvalidToken)?;

        if claims.token_type != TokenType::Access {
            return Err(AuthError::InvalidToken);
        }

        let user_id: Snowflake = claims
            .sub
            .parse()
            .map_err(|_| AuthError::InvalidToken)?;

        Ok(AuthUser {
            id: user_id,
            jti: claims.jti,
        })
    }
}

/// Optional auth — returns None for unauthenticated requests
#[derive(Debug, Clone)]
pub struct MaybeAuthUser(pub Option<AuthUser>);

impl<S> FromRequestParts<Arc<S>> for MaybeAuthUser
where
    S: AuthState,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<S>,
    ) -> Result<Self, Self::Rejection> {
        match AuthUser::from_request_parts(parts, state).await {
            Ok(user) => Ok(MaybeAuthUser(Some(user))),
            Err(_) => Ok(MaybeAuthUser(None)),
        }
    }
}

fn extract_bearer_token(parts: &Parts) -> Result<String, AuthError> {
    let header = parts
        .headers
        .get(header::AUTHORIZATION)
        .ok_or(AuthError::MissingToken)?
        .to_str()
        .map_err(|_| AuthError::InvalidToken)?;

    if !header.starts_with("Bearer ") {
        return Err(AuthError::InvalidToken);
    }

    Ok(header[7..].to_string())
}

#[derive(Debug)]
pub enum AuthError {
    MissingToken,
    InvalidToken,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AuthError::MissingToken => (StatusCode::UNAUTHORIZED, "Missing authorization token"),
            AuthError::InvalidToken => (StatusCode::UNAUTHORIZED, "Invalid or expired token"),
        };

        (
            status,
            Json(json!({
                "error": {
                    "code": "UNAUTHORIZED",
                    "message": message
                }
            })),
        )
            .into_response()
    }
}
