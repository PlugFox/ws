use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum RefreshTokenError {
    #[error("Invalid refresh token encoding")]
    InvalidEncoding,
    #[error("Invalid refresh token format")]
    InvalidFormat,
    #[error("Refresh token reuse detected; session revoked for security")]
    ReuseDetected,
    #[error("Refresh token mismatch or expired")]
    InvalidOrExpired,
}

/// Parsed structure of an opaque refresh token.
#[derive(Clone, Debug)]
pub struct RefreshToken {
    pub session_id: Uuid,
    pub generation: u32,
    pub entropy: [u8; 32],
}

impl RefreshToken {
    /// Generates a new fresh refresh token for a session at generation 0.
    pub fn generate(session_id: Uuid) -> (Self, String, [u8; 32]) {
        Self::generate_with_generation(session_id, 0)
    }

    /// Generates the next refresh token in the rotation chain with an incremented generation.
    pub fn generate_with_generation(session_id: Uuid, generation: u32) -> (Self, String, [u8; 32]) {
        let mut entropy = [0u8; 32];
        OsRng.fill_bytes(&mut entropy);

        let token = Self {
            session_id,
            generation,
            entropy,
        };

        let encoded = token.encode();
        let hash = Self::compute_hash(&encoded);
        (token, encoded, hash)
    }

    /// Encodes the token into a URL-safe Base64 string without padding.
    /// Wire format: 16 bytes session UUID + 4 bytes generation (BE) + 32 bytes entropy = 52 bytes.
    pub fn encode(&self) -> String {
        let mut raw = Vec::with_capacity(52);
        raw.extend_from_slice(self.session_id.as_bytes());
        raw.extend_from_slice(&self.generation.to_be_bytes());
        raw.extend_from_slice(&self.entropy);
        URL_SAFE_NO_PAD.encode(raw)
    }

    /// Decodes an opaque refresh token string.
    pub fn decode(encoded: &str) -> Result<Self, RefreshTokenError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| RefreshTokenError::InvalidEncoding)?;

        if bytes.len() != 52 {
            return Err(RefreshTokenError::InvalidFormat);
        }

        let session_id = Uuid::from_slice(&bytes[0..16])
            .map_err(|_| RefreshTokenError::InvalidFormat)?;

        let mut gen_bytes = [0u8; 4];
        gen_bytes.copy_from_slice(&bytes[16..20]);
        let generation = u32::from_be_bytes(gen_bytes);

        let mut entropy = [0u8; 32];
        entropy.copy_from_slice(&bytes[20..52]);

        Ok(Self {
            session_id,
            generation,
            entropy,
        })
    }

    /// Computes the SHA-256 digest of the raw token string for secure DB storage.
    pub fn compute_hash(token_str: &str) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(token_str.as_bytes());
        hasher.finalize().into()
    }

    /// Verifies two 32-byte hashes using constant-time comparison to prevent timing attacks.
    pub fn verify_hash(expected_hash: &[u8], actual_hash: &[u8]) -> bool {
        if expected_hash.len() != 32 || actual_hash.len() != 32 {
            return false;
        }
        expected_hash.ct_eq(actual_hash).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_refresh_token_encode_decode() {
        let session_id = Uuid::new_v4();
        let (token, encoded, hash) = RefreshToken::generate(session_id);

        assert_eq!(token.generation, 0);
        assert_eq!(RefreshToken::compute_hash(&encoded), hash);

        let decoded = RefreshToken::decode(&encoded).expect("Decode should succeed");
        assert_eq!(decoded.session_id, session_id);
        assert_eq!(decoded.generation, 0);
        assert_eq!(decoded.entropy, token.entropy);
    }

    #[test]
    fn test_constant_time_hash_verification() {
        let h1 = [1u8; 32];
        let h2 = [1u8; 32];
        let h3 = [2u8; 32];

        assert!(RefreshToken::verify_hash(&h1, &h2));
        assert!(!RefreshToken::verify_hash(&h1, &h3));
    }
}
