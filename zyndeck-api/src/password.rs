use argon2::password_hash::SaltString;
use argon2::password_hash::rand_core::OsRng;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};

/// Hashes a plaintext password with Argon2 (random salt), returning the encoded
/// PHC string suitable for storage. The plaintext is never persisted.
pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|error| anyhow::anyhow!("failed to hash password: {error}"))?
        .to_string();
    Ok(hash)
}

/// Verifies a plaintext password against a stored PHC hash. Returns `false` for
/// a mismatch or an unparseable hash (never panics, never leaks the reason).
pub fn verify_password(password: &str, hash: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}
