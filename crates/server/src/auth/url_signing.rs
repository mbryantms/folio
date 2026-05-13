//! HMAC-SHA256 signed URLs for OPDS-PSE page streaming (M5, audit P0).
//!
//! Most OPDS clients (Chunky, KyBook, KOReader) want a single URL template
//! per issue that they can substitute `{pageNumber}` into client-side per
//! the spec. That means the signature **cannot** include the page index —
//! one signed URL grants the holder access to *every* page of the issue
//! for the TTL window. That's fine: the underlying authorisation is "you
//! can read this issue", not "you can read page N of this issue", and the
//! per-page ACL would be identical for every page anyway.
//!
//! URL shape: `/opds/pse/{issue_id}/{n}?u={user_id}&exp={unix_ts}&sig={hex}`
//!
//! Signature canonical form (joined with `|` so the segments stay
//! unambiguous — issue ids are BLAKE3 hex (no pipes), uuids are
//! hyphenated, exp is decimal):
//!
//! ```text
//! "{issue_id}|{user_id}|{exp}"
//! ```
//!
//! The MAC key is `secrets.url_signing_key` (32 random bytes generated at
//! boot and persisted under `/data/secrets/url-signing.key`). Rotating
//! the key invalidates every outstanding signed URL — `docs/install/
//! secrets-backup.md` calls this out for operators.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

/// 30 minutes. Long enough for a Chunky/KOReader session to stream a full
/// issue at a leisurely page-flip cadence; short enough that a leaked URL
/// stops working before any practical attacker can do much with it.
pub const PSE_URL_TTL_SECS: u64 = 1800;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PseUrlError {
    #[error("PSE URL expired")]
    Expired,
    #[error("PSE URL signature invalid")]
    BadSig,
    #[error("PSE URL malformed")]
    Malformed,
}

/// Build the canonical signing payload. Pulled out so issue + verify
/// produce byte-identical inputs.
fn payload(issue_id: &str, user_id: Uuid, exp: u64) -> Vec<u8> {
    format!("{issue_id}|{user_id}|{exp}").into_bytes()
}

fn sign(issue_id: &str, user_id: Uuid, exp: u64, key: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(&payload(issue_id, user_id, exp));
    let bytes = mac.finalize().into_bytes();
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes.iter() {
        let _ = std::fmt::Write::write_fmt(&mut out, format_args!("{b:02x}"));
    }
    out
}

/// Compose a signed query string of the form `u=…&exp=…&sig=…` valid for
/// the configured TTL. The caller assembles the full path: e.g.
/// `/opds/pse/{issue_id}/{pageNumber}?{query}` with `{pageNumber}` left
/// as a literal substitution token for the OPDS client.
pub fn issue_query(issue_id: &str, user_id: Uuid, key: &[u8]) -> String {
    let exp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        + PSE_URL_TTL_SECS;
    let sig = sign(issue_id, user_id, exp, key);
    format!("u={user_id}&exp={exp}&sig={sig}")
}

/// Verify a signed URL's components. `exp` is the unix timestamp the
/// caller parsed from the `exp` query parameter; `sig_hex` is the
/// `sig` parameter (hex-encoded). Returns `Ok(())` on a fresh, untampered
/// URL; the error variants distinguish expiry vs tamper vs malformed so
/// the handler can map them to the right HTTP status.
pub fn verify(
    issue_id: &str,
    user_id: Uuid,
    exp: u64,
    sig_hex: &str,
    key: &[u8],
) -> Result<(), PseUrlError> {
    // Hex decode into a fixed-size buffer; HmacSha256 emits 32 bytes.
    if sig_hex.len() != 64 {
        return Err(PseUrlError::Malformed);
    }
    let mut sig_bytes = [0u8; 32];
    for (i, chunk) in sig_hex.as_bytes().chunks(2).enumerate() {
        let s = std::str::from_utf8(chunk).map_err(|_| PseUrlError::Malformed)?;
        sig_bytes[i] = u8::from_str_radix(s, 16).map_err(|_| PseUrlError::Malformed)?;
    }

    // Constant-time MAC verification via the hmac crate's `verify_slice`.
    let mut verifier = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    verifier.update(&payload(issue_id, user_id, exp));
    verifier
        .verify_slice(&sig_bytes)
        .map_err(|_| PseUrlError::BadSig)?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if now > exp {
        return Err(PseUrlError::Expired);
    }
    // Bound future expiry to twice the TTL — anything beyond that suggests
    // the URL was issued with a tampered key or a clock far in the future.
    if exp > now + (PSE_URL_TTL_SECS * 2) {
        return Err(PseUrlError::BadSig);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> [u8; 32] {
        [42; 32]
    }

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[test]
    fn round_trip() {
        let issue_id = "abc123";
        let user_id = Uuid::now_v7();
        let query = issue_query(issue_id, user_id, &key());
        // Parse back the components.
        let mut sig = "";
        let mut exp_s = "";
        for kv in query.split('&') {
            if let Some(v) = kv.strip_prefix("sig=") {
                sig = v;
            } else if let Some(v) = kv.strip_prefix("exp=") {
                exp_s = v;
            }
        }
        let exp: u64 = exp_s.parse().unwrap();
        assert!(verify(issue_id, user_id, exp, sig, &key()).is_ok());
    }

    #[test]
    fn tampered_sig_rejected() {
        let issue_id = "abc123";
        let user_id = Uuid::now_v7();
        let exp = now() + 60;
        let mut sig = sign(issue_id, user_id, exp, &key());
        // Flip the last hex nibble.
        let last = sig.pop().unwrap();
        let flipped = if last == 'f' { '0' } else { 'f' };
        sig.push(flipped);
        assert_eq!(
            verify(issue_id, user_id, exp, &sig, &key()),
            Err(PseUrlError::BadSig)
        );
    }

    #[test]
    fn wrong_user_id_rejected() {
        let issue_id = "abc123";
        let user_a = Uuid::now_v7();
        let user_b = Uuid::now_v7();
        let exp = now() + 60;
        let sig = sign(issue_id, user_a, exp, &key());
        // Verifying as user_b with user_a's signature fails (payload
        // includes the uuid so any swap changes the MAC).
        assert_eq!(
            verify(issue_id, user_b, exp, &sig, &key()),
            Err(PseUrlError::BadSig)
        );
    }

    #[test]
    fn wrong_issue_id_rejected() {
        let user_id = Uuid::now_v7();
        let exp = now() + 60;
        let sig = sign("issue-a", user_id, exp, &key());
        assert_eq!(
            verify("issue-b", user_id, exp, &sig, &key()),
            Err(PseUrlError::BadSig)
        );
    }

    #[test]
    fn expired_rejected() {
        let issue_id = "abc123";
        let user_id = Uuid::now_v7();
        let past = now().saturating_sub(60);
        let sig = sign(issue_id, user_id, past, &key());
        assert_eq!(
            verify(issue_id, user_id, past, &sig, &key()),
            Err(PseUrlError::Expired)
        );
    }

    #[test]
    fn malformed_sig_rejected() {
        let issue_id = "abc123";
        let user_id = Uuid::now_v7();
        let exp = now() + 60;
        // Too short.
        assert_eq!(
            verify(issue_id, user_id, exp, "deadbeef", &key()),
            Err(PseUrlError::Malformed)
        );
        // Non-hex characters at correct length.
        let zz = "z".repeat(64);
        assert_eq!(
            verify(issue_id, user_id, exp, &zz, &key()),
            Err(PseUrlError::Malformed)
        );
    }

    #[test]
    fn wrong_key_rejected() {
        let issue_id = "abc123";
        let user_id = Uuid::now_v7();
        let exp = now() + 60;
        let sig = sign(issue_id, user_id, exp, &key());
        let other = [7u8; 32];
        assert_eq!(
            verify(issue_id, user_id, exp, &sig, &other),
            Err(PseUrlError::BadSig)
        );
    }

    #[test]
    fn extreme_future_exp_rejected() {
        let issue_id = "abc123";
        let user_id = Uuid::now_v7();
        // 1 year in the future — well beyond 2× TTL.
        let exp = now() + 31_536_000;
        let sig = sign(issue_id, user_id, exp, &key());
        assert_eq!(
            verify(issue_id, user_id, exp, &sig, &key()),
            Err(PseUrlError::BadSig)
        );
    }
}
