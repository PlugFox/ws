use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PasswordError {
    #[error("Failed to hash password: {0}")]
    HashError(String),
    #[error("Invalid password hash format")]
    InvalidHash,
    #[error("Password verification failed")]
    VerificationFailed,
}

/// Hashes a plaintext password using Argon2id with a cryptographically secure random salt.
pub fn hash_password(password: &str) -> Result<String, PasswordError> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| PasswordError::HashError(e.to_string()))
}

/// Verifies a plaintext password against an Argon2id password hash string.
pub fn verify_password(password: &str, password_hash_str: &str) -> Result<bool, PasswordError> {
    let parsed_hash =
        PasswordHash::new(password_hash_str).map_err(|_| PasswordError::InvalidHash)?;
    let argon2 = Argon2::default();
    match argon2.verify_password(password.as_bytes(), &parsed_hash) {
        Ok(_) => Ok(true),
        Err(argon2::password_hash::Error::Password) => Ok(false),
        Err(err) => Err(PasswordError::HashError(err.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_password_hash_and_verify() {
        let password = "SuperSecretPassword123!";
        let hash = hash_password(password).expect("Hashing should succeed");
        assert!(hash.starts_with("$argon2id$"));

        let is_valid = verify_password(password, &hash).expect("Verification should succeed");
        assert!(is_valid);

        let is_invalid = verify_password("WrongPassword!", &hash).expect("Verification should succeed");
        assert!(!is_invalid);
    }
}
