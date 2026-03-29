use chrono::Utc;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use lumiere_models::snowflake::Snowflake;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: u64,
    pub iat: u64,
    pub jti: String,
    pub token_type: TokenType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenType {
    Access,
    Refresh,
}

pub fn create_access_token(
    user_id: Snowflake,
    secret: &str,
    ttl_seconds: u64,
) -> anyhow::Result<(String, String)> {
    create_token(user_id, secret, ttl_seconds, TokenType::Access)
}

pub fn create_refresh_token(
    user_id: Snowflake,
    secret: &str,
    ttl_seconds: u64,
) -> anyhow::Result<(String, String)> {
    create_token(user_id, secret, ttl_seconds, TokenType::Refresh)
}

fn create_token(
    user_id: Snowflake,
    secret: &str,
    ttl_seconds: u64,
    token_type: TokenType,
) -> anyhow::Result<(String, String)> {
    let now = Utc::now().timestamp() as u64;
    let jti = nanoid();

    let claims = Claims {
        sub: user_id.to_string(),
        exp: now + ttl_seconds,
        iat: now,
        jti: jti.clone(),
        token_type,
    };

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;

    Ok((token, jti))
}

pub fn verify_token(token: &str, secret: &str) -> anyhow::Result<Claims> {
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )?;

    Ok(token_data.claims)
}

fn nanoid() -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut rng = rand::thread_rng();
    (0..21)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_verify_access_token() {
        let user_id = Snowflake(123456789);
        let secret = "test_secret_key_for_testing";

        let (token, jti) = create_access_token(user_id, secret, 900).unwrap();
        assert!(!token.is_empty());
        assert!(!jti.is_empty());

        let claims = verify_token(&token, secret).unwrap();
        assert_eq!(claims.sub, "123456789");
        assert_eq!(claims.jti, jti);
        assert_eq!(claims.token_type, TokenType::Access);
    }

    #[test]
    fn test_expired_token() {
        let user_id = Snowflake(123456789);
        let secret = "test_secret";

        // Manually create an expired token by setting exp in the past
        let now = chrono::Utc::now().timestamp() as u64;
        let claims = Claims {
            sub: user_id.to_string(),
            exp: now - 100, // 100 seconds in the past
            iat: now - 200,
            jti: "test".to_string(),
            token_type: TokenType::Access,
        };
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap();

        assert!(verify_token(&token, secret).is_err());
    }

    #[test]
    fn test_wrong_secret() {
        let user_id = Snowflake(123456789);
        let (token, _) = create_access_token(user_id, "secret1", 900).unwrap();
        assert!(verify_token(&token, "secret2").is_err());
    }
}
