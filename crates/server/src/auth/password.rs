//! argon2id password hashing with a server-side pepper (§17.1).
//!
//! Construction: argon2id(password || pepper, salt) with parameters
//!   m=64 MiB, t=3, p=1
//! per the spec. The pepper lives in `/data/secrets/pepper` and is loaded
//! at startup; it never appears in stored hashes (so a DB-only leak doesn't
//! enable offline attack — the attacker also needs filesystem access).
//!
//! The PHC string written to the DB looks like:
//!   $argon2id$v=19$m=65536,t=3,p=1$<salt-base64>$<hash-base64>

use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};
use rand::rngs::OsRng;

#[derive(Debug, thiserror::Error)]
pub enum PasswordError {
    #[error("argon2 error: {0}")]
    Argon2(String),
    #[error("invalid stored hash")]
    InvalidHash,
}

fn argon2_with_pepper(pepper: &[u8]) -> argon2::Argon2<'_> {
    let params = Params::new(
        64 * 1024, // m_cost in KiB → 64 MiB
        3,         // t_cost
        1,         // p_cost
        None,      // output length (default 32)
    )
    .expect("valid argon2 params");
    Argon2::new_with_secret(pepper, Algorithm::Argon2id, Version::V0x13, params)
        .expect("valid argon2 secret")
}

pub fn hash(plain: &str, pepper: &[u8]) -> Result<String, PasswordError> {
    let salt = SaltString::generate(&mut OsRng);
    let argon = argon2_with_pepper(pepper);
    Ok(argon
        .hash_password(plain.as_bytes(), &salt)
        .map_err(|e| PasswordError::Argon2(e.to_string()))?
        .to_string())
}

pub fn verify(stored_hash: &str, plain: &str, pepper: &[u8]) -> Result<bool, PasswordError> {
    let parsed = PasswordHash::new(stored_hash).map_err(|_| PasswordError::InvalidHash)?;
    let argon = argon2_with_pepper(pepper);
    Ok(argon.verify_password(plain.as_bytes(), &parsed).is_ok())
}

/// PHC string of a real argon2id hash, computed once per process. Used by
/// the login handler on the missing-user path so the response time matches
/// the wrong-password path (both run a real verify). Without this the
/// timing channel reliably distinguishes "no user" from "wrong password"
/// because the previous malformed dummy literal failed `PasswordHash::new`
/// instantly with no argon2 work.
///
/// We hash a fixed throwaway plaintext under a fresh random salt; the
/// resulting PHC string is itself meaningless — what matters is that
/// `verify` runs the full m=64MiB / t=3 / p=1 argon2id work on it.
pub fn dummy_hash(pepper: &[u8]) -> &'static str {
    use std::sync::OnceLock;
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| {
        // The exact plaintext doesn't matter; this hash is never compared
        // against anything that could verify true.
        hash("dummy-for-constant-time-login", pepper)
            .expect("argon2 hash succeeds with valid params")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let pepper = b"test-pepper-32-bytes-long-XXXXXX";
        let h = hash("hunter2", pepper).unwrap();
        assert!(verify(&h, "hunter2", pepper).unwrap());
        assert!(!verify(&h, "wrong", pepper).unwrap());
    }

    #[test]
    fn pepper_changes_invalidate() {
        let h = hash("hunter2", b"pepper-A-32bytes-XXXXXXXXXXXXXXX").unwrap();
        // Same password, different pepper → must not verify (peppered hash).
        assert!(!verify(&h, "hunter2", b"pepper-B-32bytes-XXXXXXXXXXXXXXX").unwrap());
    }
}
