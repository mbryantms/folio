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
    /// The blocking task that ran the hash or verify failed (it panicked or
    /// was cancelled). Says nothing about the password: callers answer 500,
    /// never "wrong password" and never "correct".
    #[error("password task failed: {0}")]
    Task(String),
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

// ─────────────── async call sites: the blocking pool ───────────────
//
// One argon2 hash or verify is tens of milliseconds of CPU and tens of MiB
// of memory. Run on an async worker thread it stalls every other request
// scheduled there, so async handlers go through these wrappers, which move
// the work onto tokio's blocking pool. The pepper and the plaintext are
// copied into zeroizing buffers because the task must own its inputs.
//
// The blocking pool itself is large (hundreds of threads) and shared with
// page, thumbnail and OPDS streaming work, so a process-wide semaphore
// caps how many hashes run at once. With `hash_permits()` permits this
// restores the bound inline hashing had: at most one hash per core.
// Waiting callers queue; there is no timeout.

/// How many argon2 hashes or verifies may run at once: the available
/// parallelism (one per core, as when hashing ran inline on the async
/// workers), at least 2.
pub fn hash_permits() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(2)
        .max(2)
}

/// The process-wide gate in front of every blocking hash or verify.
fn hash_gate() -> &'static std::sync::Arc<tokio::sync::Semaphore> {
    static GATE: std::sync::OnceLock<std::sync::Arc<tokio::sync::Semaphore>> =
        std::sync::OnceLock::new();
    GATE.get_or_init(|| std::sync::Arc::new(tokio::sync::Semaphore::new(hash_permits())))
}

/// Owned copy of a [`Peppers`] set for a blocking task.
struct OwnedPeppers {
    current: zeroize::Zeroizing<Vec<u8>>,
    previous: Option<zeroize::Zeroizing<Vec<u8>>>,
}

impl OwnedPeppers {
    fn from(p: Peppers<'_>) -> Self {
        Self {
            current: zeroize::Zeroizing::new(p.current.to_vec()),
            previous: p.previous.map(|v| zeroize::Zeroizing::new(v.to_vec())),
        }
    }
    fn borrow(&self) -> Peppers<'_> {
        Peppers {
            current: &self.current,
            previous: self.previous.as_deref().map(|v| v.as_slice()),
        }
    }
}

/// Run `f` on the blocking pool once a [`hash_gate`] permit is free. The
/// permit moves into the task, so it is released when the work ends, also
/// on a panic and when the awaiting request has gone away meanwhile.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, PasswordError> + Send + 'static,
) -> Result<T, PasswordError> {
    let permit = hash_gate()
        .clone()
        .acquire_owned()
        .await
        .map_err(|e| PasswordError::Task(e.to_string()))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        f()
    })
    .await
    .map_err(|e| PasswordError::Task(e.to_string()))?
}

/// [`hash`] on the blocking pool.
pub async fn hash_blocking(
    plain: &str,
    pepper: &[u8],
    cost: HashCost,
) -> Result<String, PasswordError> {
    let plain = zeroize::Zeroizing::new(plain.to_owned());
    let pepper = zeroize::Zeroizing::new(pepper.to_vec());
    blocking(move || hash(&plain, &pepper, cost)).await
}

/// [`verify_rotating`] on the blocking pool.
pub async fn verify_rotating_blocking(
    stored_hash: &str,
    plain: &str,
    peppers: Peppers<'_>,
) -> Result<Verified, PasswordError> {
    let stored = stored_hash.to_owned();
    let plain = zeroize::Zeroizing::new(plain.to_owned());
    let peppers = OwnedPeppers::from(peppers);
    blocking(move || verify_rotating(&stored, &plain, peppers.borrow())).await
}

/// The timing equalizer for a sign-in that has no hash to check (unknown
/// address, or an account without a local password): the same
/// one-or-two argon2 verifies a wrong password costs, against
/// [`dummy_hash`], on the blocking pool. Never verifies; `Err` only when
/// the task itself failed ([`PasswordError::Task`]).
pub async fn dummy_verify_blocking(
    plain: &str,
    peppers: Peppers<'_>,
    cost: HashCost,
) -> Result<(), PasswordError> {
    let plain = zeroize::Zeroizing::new(plain.to_owned());
    let peppers = OwnedPeppers::from(peppers);
    blocking(move || {
        let dummy = dummy_hash(&peppers.current, cost);
        let _ = verify_rotating(dummy, &plain, peppers.borrow());
        Ok(())
    })
    .await
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

    /// The blocking-pool wrappers give the same answers as the sync calls.
    #[tokio::test]
    async fn blocking_wrappers_match_the_sync_calls() {
        let old = b"pepper-A-32bytes-XXXXXXXXXXXXXXX";
        let new = b"pepper-B-32bytes-XXXXXXXXXXXXXXX";
        let h = hash_blocking("hunter2", new, HashCost::TEST).await.unwrap();
        assert!(verify(&h, "hunter2", new).unwrap());
        let under_old = hash("hunter2", old, HashCost::TEST).unwrap();
        let rotating = Peppers {
            current: new,
            previous: Some(old),
        };
        assert_eq!(
            verify_rotating_blocking(&h, "hunter2", rotating)
                .await
                .unwrap(),
            Verified::Current
        );
        assert_eq!(
            verify_rotating_blocking(&under_old, "hunter2", rotating)
                .await
                .unwrap(),
            Verified::Previous
        );
        assert_eq!(
            verify_rotating_blocking(&h, "wrong", rotating)
                .await
                .unwrap(),
            Verified::No
        );
        dummy_verify_blocking("anything", rotating, HashCost::TEST)
            .await
            .unwrap();
    }

    /// The gate bounds concurrency: with every permit held a hash waits,
    /// and it proceeds once they are released.
    #[tokio::test]
    async fn hash_gate_bounds_concurrent_hashes() {
        assert!(hash_permits() >= 2);
        let held = hash_gate()
            .clone()
            .acquire_many_owned(u32::try_from(hash_permits()).unwrap())
            .await
            .unwrap();
        let pepper = b"test-pepper-32-bytes-long-XXXXXX";
        let mut waiting = tokio::spawn(hash_blocking("hunter2", pepper, HashCost::TEST));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut waiting)
                .await
                .is_err(),
            "a hash ran while every permit was held"
        );
        drop(held);
        let h = waiting.await.unwrap().unwrap();
        assert!(verify(&h, "hunter2", pepper).unwrap());
    }

    /// A failed task is an error of its own, never a verdict on the password.
    #[tokio::test]
    async fn a_failed_task_is_not_a_verdict() {
        let r = blocking::<Verified>(|| panic!("boom")).await;
        assert!(matches!(r, Err(PasswordError::Task(_))), "{r:?}");
    }
}
