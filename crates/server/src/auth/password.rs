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
//!
//! The cost of NEW hashes is a [`HashCost`] carried on `Config`
//! (`password_hash_cost`): always [`HashCost::PRODUCTION`] in a real
//! server, never read from the environment. Only the integration-test
//! harness lowers it to [`HashCost::TEST`] — the suite registers and logs
//! in hundreds of users, and at 64 MiB × 3 passes those hashes dominated
//! CI. Verification needs no cost: argon2 reads m/t/p from the stored PHC
//! string, so hashes of either cost verify under either setting.

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

/// argon2id cost parameters for newly written hashes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HashCost {
    /// Memory in KiB.
    pub m_kib: u32,
    /// Passes.
    pub t: u32,
    /// Lanes.
    pub p: u32,
}

impl HashCost {
    /// The spec's parameters (§17.1): m=64 MiB, t=3, p=1.
    pub const PRODUCTION: Self = Self {
        m_kib: 64 * 1024,
        t: 3,
        p: 1,
    };
    /// Integration tests only (set by the `TestApp` harness). Same
    /// algorithm and code path at ~1/24th the work; never a production
    /// setting — `Config::load()` cannot produce it.
    pub const TEST: Self = Self {
        m_kib: 8 * 1024,
        t: 1,
        p: 1,
    };
}

impl Default for HashCost {
    fn default() -> Self {
        Self::PRODUCTION
    }
}

fn argon2_with_pepper(pepper: &[u8], cost: HashCost) -> argon2::Argon2<'_> {
    let params = Params::new(cost.m_kib, cost.t, cost.p, None).expect("valid argon2 params");
    Argon2::new_with_secret(pepper, Algorithm::Argon2id, Version::V0x13, params)
        .expect("valid argon2 secret")
}

pub fn hash(plain: &str, pepper: &[u8], cost: HashCost) -> Result<String, PasswordError> {
    let argon = argon2_with_pepper(pepper, cost);
    // password-hash 0.6: `hash_password` generates the 16-byte random salt
    // itself (getrandom); the explicit SaltString/OsRng dance is gone.
    Ok(argon
        .hash_password(plain.as_bytes())
        .map_err(|e| PasswordError::Argon2(e.to_string()))?
        .to_string())
}

pub fn verify(stored_hash: &str, plain: &str, pepper: &[u8]) -> Result<bool, PasswordError> {
    let parsed = PasswordHash::new(stored_hash).map_err(|_| PasswordError::InvalidHash)?;
    // The instance cost is irrelevant here: `verify_password` uses the
    // m/t/p encoded in `parsed`.
    let argon = argon2_with_pepper(pepper, HashCost::PRODUCTION);
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
/// `verify` runs the same argon2id work on it as on a real user's hash.
/// `cost` must therefore be the cost real hashes are written at; the first
/// call fixes it for the process (it is process-wide config anyway).
pub fn dummy_hash(pepper: &[u8], cost: HashCost) -> &'static str {
    use std::sync::OnceLock;
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| {
        // The exact plaintext doesn't matter; this hash is never compared
        // against anything that could verify true.
        hash("dummy-for-constant-time-login", pepper, cost)
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
        let h = hash("hunter2", pepper, HashCost::TEST).unwrap();
        assert!(verify(&h, "hunter2", pepper).unwrap());
        assert!(!verify(&h, "wrong", pepper).unwrap());
    }

    #[test]
    fn rotating_verify_prefers_current_then_previous() {
        let old = b"pepper-A-32bytes-XXXXXXXXXXXXXXX";
        let new = b"pepper-B-32bytes-XXXXXXXXXXXXXXX";
        let under_old = hash("hunter2", old, HashCost::TEST).unwrap();
        let under_new = hash("hunter2", new, HashCost::TEST).unwrap();
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
        let rehashed = hash("hunter2", rotating.current, HashCost::TEST).unwrap();
        assert_eq!(
            verify_rotating(&rehashed, "hunter2", Peppers::single(new)).unwrap(),
            Verified::Current
        );
    }

    /// Production cost must never drift: stored hashes and the login
    /// timing equalizer both assume the spec's parameters.
    #[test]
    fn production_cost_is_the_spec() {
        let pepper = b"test-pepper-32-bytes-long-XXXXXX";
        let h = hash("hunter2", pepper, HashCost::PRODUCTION).unwrap();
        assert!(h.contains("$m=65536,t=3,p=1$"), "{h}");
        assert_eq!(HashCost::default(), HashCost::PRODUCTION);
    }

    /// Verification reads the cost from the stored hash, so a test-cost
    /// hash and a production-cost hash both verify through `verify`.
    #[test]
    fn verify_uses_the_stored_cost() {
        let pepper = b"test-pepper-32-bytes-long-XXXXXX";
        let cheap = hash("hunter2", pepper, HashCost::TEST).unwrap();
        assert!(cheap.contains("$m=8192,t=1,p=1$"), "{cheap}");
        assert!(verify(&cheap, "hunter2", pepper).unwrap());
        assert!(!verify(&cheap, "wrong", pepper).unwrap());
    }

    #[test]
    fn pepper_changes_invalidate() {
        let h = hash(
            "hunter2",
            b"pepper-A-32bytes-XXXXXXXXXXXXXXX",
            HashCost::TEST,
        )
        .unwrap();
        // Same password, different pepper → must not verify (peppered hash).
        assert!(!verify(&h, "hunter2", b"pepper-B-32bytes-XXXXXXXXXXXXXXX").unwrap());
    }
}
