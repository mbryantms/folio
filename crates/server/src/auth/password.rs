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
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};

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
    let argon = argon2_with_pepper(pepper);
    // password-hash 0.6: `hash_password` generates the 16-byte random salt
    // itself (getrandom); the explicit SaltString/OsRng dance is gone.
    Ok(argon
        .hash_password(plain.as_bytes())
        .map_err(|e| PasswordError::Argon2(e.to_string()))?
        .to_string())
}

pub fn verify(stored_hash: &str, plain: &str, pepper: &[u8]) -> Result<bool, PasswordError> {
    let parsed = PasswordHash::new(stored_hash).map_err(|_| PasswordError::InvalidHash)?;
    let argon = argon2_with_pepper(pepper);
    Ok(argon.verify_password(plain.as_bytes(), &parsed).is_ok())
}

/// The pepper set a verify runs against (security audit L-1, WP-6.3).
///
/// `current` is `secrets/pepper` — every new hash is written under it.
/// `previous` is the optional `secrets/pepper.previous`, present only while a
/// pepper rotation is in progress: hashes written before the rotation still
/// verify against it, and the caller rehashes them under `current` on the
/// spot (verify-and-rehash). Once the operator deletes `pepper.previous`, any
/// hash that never got rehashed stops verifying and that user goes through
/// `/forgot-password`. Runbook: `docs/install/secrets-backup.md`.
#[derive(Clone, Copy)]
pub struct Peppers<'a> {
    pub current: &'a [u8],
    pub previous: Option<&'a [u8]>,
}

impl<'a> Peppers<'a> {
    /// A single, non-rotating pepper.
    pub fn single(current: &'a [u8]) -> Self {
        Self {
            current,
            previous: None,
        }
    }
}

/// Outcome of [`verify_rotating`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verified {
    /// Matched under the current pepper — nothing to do.
    Current,
    /// Matched under the previous pepper — the caller should rehash the
    /// plaintext with [`hash`] under the current pepper and persist it.
    Previous,
    /// No match.
    No,
}

impl Verified {
    pub fn ok(self) -> bool {
        !matches!(self, Self::No)
    }
}

/// Verify against the current pepper, then (when a rotation is in progress)
/// the previous one. Runs the second argon2 verify only on a current-pepper
/// miss, so steady-state logins cost one verify; callers that need constant
/// time across "no user" vs "wrong password" (the login dummy path) call this
/// too, so both paths pay the same one-or-two verifies.
pub fn verify_rotating(
    stored_hash: &str,
    plain: &str,
    peppers: Peppers<'_>,
) -> Result<Verified, PasswordError> {
    if verify(stored_hash, plain, peppers.current)? {
        return Ok(Verified::Current);
    }
    if let Some(prev) = peppers.previous
        && verify(stored_hash, plain, prev)?
    {
        return Ok(Verified::Previous);
    }
    Ok(Verified::No)
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

    /// Stored-hash back-compat: this PHC string was generated by argon2 0.5
    /// (password-hash 0.5 `SaltString`) with the same pepper/params. Hashes
    /// already in users' rows must keep verifying across crate upgrades.
    #[test]
    fn verifies_hash_produced_by_argon2_0_5() {
        let pepper = b"test-pepper-32-bytes-long-XXXXXX";
        let legacy = "$argon2id$v=19$m=65536,t=3,p=1$IM3ZKBQz7nYPT6+dBWAqkQ$MUy0Y+RRDDZf0/THuux8iUA4/bVTt7QpiyYrZLRamhU";
        assert!(verify(legacy, "hunter2", pepper).unwrap());
        assert!(!verify(legacy, "wrong", pepper).unwrap());
    }

    #[test]
    fn round_trip() {
        let pepper = b"test-pepper-32-bytes-long-XXXXXX";
        let h = hash("hunter2", pepper).unwrap();
        assert!(verify(&h, "hunter2", pepper).unwrap());
        assert!(!verify(&h, "wrong", pepper).unwrap());
    }

    #[test]
    fn rotating_verify_prefers_current_then_previous() {
        let old = b"pepper-A-32bytes-XXXXXXXXXXXXXXX";
        let new = b"pepper-B-32bytes-XXXXXXXXXXXXXXX";
        let under_old = hash("hunter2", old).unwrap();
        let under_new = hash("hunter2", new).unwrap();
        let rotating = Peppers {
            current: new,
            previous: Some(old),
        };
        assert_eq!(
            verify_rotating(&under_new, "hunter2", rotating).unwrap(),
            Verified::Current
        );
        assert_eq!(
            verify_rotating(&under_old, "hunter2", rotating).unwrap(),
            Verified::Previous
        );
        assert_eq!(
            verify_rotating(&under_old, "wrong", rotating).unwrap(),
            Verified::No
        );
        // Rotation finished (previous dropped): old hashes stop verifying.
        assert_eq!(
            verify_rotating(&under_old, "hunter2", Peppers::single(new)).unwrap(),
            Verified::No
        );
        // The rehash a caller writes on `Previous` verifies under current.
        let rehashed = hash("hunter2", rotating.current).unwrap();
        assert_eq!(
            verify_rotating(&rehashed, "hunter2", Peppers::single(new)).unwrap(),
            Verified::Current
        );
    }

    #[test]
    fn pepper_changes_invalidate() {
        let h = hash("hunter2", b"pepper-A-32bytes-XXXXXXXXXXXXXXX").unwrap();
        // Same password, different pepper → must not verify (peppered hash).
        assert!(!verify(&h, "hunter2", b"pepper-B-32bytes-XXXXXXXXXXXXXXX").unwrap());
    }
}
