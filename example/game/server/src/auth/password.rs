use argon2::{
    password_hash::{phc::PasswordHash, PasswordHasher, PasswordVerifier},
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

/// Hashes a plaintext password using Argon2id with a cryptographically secure random salt
/// (from the OS, `getrandom`).
pub fn hash_password(password: &str) -> Result<String, PasswordError> {
    let argon2 = Argon2::default();
    argon2
        .hash_password(password.as_bytes())
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
        Err(argon2::password_hash::Error::PasswordInvalid) => Ok(false),
        Err(err) => Err(PasswordError::HashError(err.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
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

    /// A hash stored by argon2 0.5 (the PHC string format did not change in
    /// 0.6): accounts made before the bump still sign in.
    #[test]
    fn a_hash_from_argon2_0_5_still_verifies() {
        const STORED: &str = "$argon2id$v=19$m=19456,t=2,p=1$BPzxxtJVkm3pLvZbaa7Bjg$l3J8TrJ4Jzt4OLBQHCLbvy9xl4mk5OgcqxF5ksY/h9U";
        assert!(verify_password("correct horse battery staple", STORED).unwrap());
        assert!(!verify_password("correct horse battery stapler", STORED).unwrap());
        assert_matches!(verify_password("x", "not a hash"), Err(PasswordError::InvalidHash));
    }
}
