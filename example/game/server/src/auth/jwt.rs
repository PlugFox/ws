use chrono::Utc;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum JwtError {
    #[error("Token serialization or signing failed: {0}")]
    EncodingError(#[from] jsonwebtoken::errors::Error),
    #[error("Token expired")]
    Expired,
    #[error("Invalid token")]
    InvalidToken,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccessTokenClaims {
    /// Account / User ID
    pub sub: u32,
    /// Session UUID
    pub sid: String,
    /// Issued at (Unix timestamp)
    pub iat: i64,
    /// Expiration time (Unix timestamp)
    pub exp: i64,
    /// Unique token ID
    pub jti: String,
}

#[derive(Clone)]
pub struct JwtManager {
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    access_ttl: Duration,
    leeway_secs: u64,
}

impl JwtManager {
    pub fn new(secret: &[u8], access_ttl: Duration) -> Self {
        Self {
            encoding_key: EncodingKey::from_secret(secret),
            decoding_key: DecodingKey::from_secret(secret),
            access_ttl,
            leeway_secs: 30,
        }
    }

    /// Issues a new signed JWT access token for a given user and session.
    pub fn issue_access_token(&self, user_id: u32, session_id: &Uuid) -> Result<(String, i64), JwtError> {
        let now = Utc::now().timestamp();
        let exp = now + self.access_ttl.as_secs() as i64;
        let claims = AccessTokenClaims {
            sub: user_id,
            sid: session_id.to_string(),
            iat: now,
            exp,
            jti: Uuid::new_v4().to_string(),
        };

        let token = encode(&Header::default(), &claims, &self.encoding_key)?;
        Ok((token, exp))
    }

    /// Verifies and decodes a JWT access token, enforcing expiration with skew leeway.
    pub fn verify_access_token(&self, token: &str) -> Result<AccessTokenClaims, JwtError> {
        let mut validation = Validation::default();
        validation.leeway = self.leeway_secs;
        validation.validate_exp = true;

        let token_data = decode::<AccessTokenClaims>(token, &self.decoding_key, &validation)
            .map_err(|err| match err.kind() {
                jsonwebtoken::errors::ErrorKind::ExpiredSignature => JwtError::Expired,
                _ => JwtError::InvalidToken,
            })?;

        Ok(token_data.claims)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_issue_and_verify_token() {
        let secret = b"my_super_secret_test_key_123456";
        let manager = JwtManager::new(secret, Duration::from_secs(60));

        let user_id = 42;
        let session_id = Uuid::new_v4();

        let (token, exp) = manager.issue_access_token(user_id, &session_id).unwrap();
        assert!(!token.is_empty());
        assert!(exp > Utc::now().timestamp());

        let claims = manager.verify_access_token(&token).unwrap();
        assert_eq!(claims.sub, user_id);
        assert_eq!(claims.sid, session_id.to_string());
    }
}
