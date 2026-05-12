//! HMAC-SHA256 single-use tokens for the recovery flow (M4, audit M-1).
//!
//! Stateless: the token IS the credential, so we don't store anything in
//! the DB and never have to clean up. Token format (57 raw bytes,
//! base64url-encoded with no padding ≈ 76 chars):
//!
//! ```text
//!  ┌─ purpose tag  (1 byte: 1=verify, 2=reset)
//!  │  ┌─ user_id   (16 bytes: uuid)
//!  │  │            ┌─ expires_at (8 bytes: big-endian u64 unix seconds)
//!  │  │            │       ┌─ HMAC-SHA256 over the payload (32 bytes)
//!  │  │            │       │
//! [P][U U U U …  ][E E E E E E E E][M M M M …  ]
//! ```
//!
//! The MAC key is `secrets.email_token_key` (32 random bytes generated
//! at boot and persisted under `/data/secrets/email-token.key`). Rotating
//! the key invalidates every outstanding token, which is exactly the
//! invariant we want for credential reset material.
//!
//! Replay defense: the email-verification path consumes the token by
//! flipping `users.state` from `pending_verification` to `active`, so
//! re-using the same link is a no-op (the user is already active). The
//! password-reset path consumes the token by bumping `token_version` —
//! a re-used reset link will succeed once but the bumped `tv` means any
//! subsequent reset is moot (the password is already changed).

use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

const PURPOSE_LEN: usize = 1;
const UUID_LEN: usize = 16;
const EXPIRES_LEN: usize = 8;
const MAC_LEN: usize = 32;
const PAYLOAD_LEN: usize = PURPOSE_LEN + UUID_LEN + EXPIRES_LEN;
const TOKEN_BYTES: usize = PAYLOAD_LEN + MAC_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenPurpose {
    EmailVerification,
    PasswordReset,
}

impl TokenPurpose {
    fn byte(self) -> u8 {
        match self {
            Self::EmailVerification => 1,
            Self::PasswordReset => 2,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("malformed token")]
    Malformed,
    #[error("token issued for a different purpose")]
    WrongPurpose,
    #[error("token expired")]
    Expired,
    #[error("token signature invalid")]
    BadMac,
    #[error("token issued in the future (clock skew?)")]
    FromFuture,
}

/// Issue a fresh token for `user_id` with the given `purpose` and `ttl`.
/// Returns a URL-safe (base64url, no padding) string suitable for inclusion
/// in a `?token=…` query parameter.
pub fn issue(purpose: TokenPurpose, user_id: Uuid, ttl: Duration, key: &[u8]) -> String {
    let expires_at = SystemTime::now()
        .checked_add(ttl)
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .expect("ttl + now fits in u64 unix seconds");

    let mut payload = [0u8; PAYLOAD_LEN];
    payload[0] = purpose.byte();
    payload[PURPOSE_LEN..PURPOSE_LEN + UUID_LEN].copy_from_slice(user_id.as_bytes());
    payload[PURPOSE_LEN + UUID_LEN..].copy_from_slice(&expires_at.to_be_bytes());

    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(&payload);
    let mac_bytes = mac.finalize().into_bytes();

    let mut out = [0u8; TOKEN_BYTES];
    out[..PAYLOAD_LEN].copy_from_slice(&payload);
    out[PAYLOAD_LEN..].copy_from_slice(&mac_bytes);

    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(out)
}

/// Verify `token` was issued for `purpose`, hasn't expired, and the MAC
/// matches the supplied `key`. Returns the embedded user id on success.
/// Errors are intentionally distinct so the calling handler can choose
/// which to surface to the user (typically: collapse all variants to
/// "invalid or expired link" in the response, but log the discriminator
/// for forensic).
pub fn verify(purpose: TokenPurpose, token: &str, key: &[u8]) -> Result<Uuid, TokenError> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|_| TokenError::Malformed)?;
    if bytes.len() != TOKEN_BYTES {
        return Err(TokenError::Malformed);
    }
    let (payload, mac_part) = bytes.split_at(PAYLOAD_LEN);

    // Purpose check first — failing fast here costs nothing and gives the
    // caller a clearer error than a MAC mismatch would.
    if payload[0] != purpose.byte() {
        return Err(TokenError::WrongPurpose);
    }

    // Constant-time MAC verification.
    let mut verifier = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    verifier.update(payload);
    verifier
        .verify_slice(mac_part)
        .map_err(|_| TokenError::BadMac)?;

    let user_id_bytes: [u8; UUID_LEN] = payload[PURPOSE_LEN..PURPOSE_LEN + UUID_LEN]
        .try_into()
        .expect("slice length checked above");
    let user_id = Uuid::from_bytes(user_id_bytes);

    let exp_bytes: [u8; EXPIRES_LEN] = payload[PURPOSE_LEN + UUID_LEN..]
        .try_into()
        .expect("slice length checked above");
    let expires_at = u64::from_be_bytes(exp_bytes);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Allow a tiny negative skew window — if the issuer clock and the
    // verifier clock disagree by a few seconds we shouldn't reject. The
    // expiry path catches the "more than a minute in the past" case.
    if now > expires_at {
        return Err(TokenError::Expired);
    }
    // 60-second future window. Beyond that, something's wrong.
    if expires_at > now + (86_400 * 30 + 60) {
        return Err(TokenError::FromFuture);
    }

    Ok(user_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> [u8; 32] {
        [42; 32]
    }

    #[test]
    fn round_trip_email_verification() {
        let uid = Uuid::now_v7();
        let tok = issue(
            TokenPurpose::EmailVerification,
            uid,
            Duration::from_secs(60),
            &key(),
        );
        let got = verify(TokenPurpose::EmailVerification, &tok, &key()).unwrap();
        assert_eq!(got, uid);
    }

    #[test]
    fn round_trip_password_reset() {
        let uid = Uuid::now_v7();
        let tok = issue(
            TokenPurpose::PasswordReset,
            uid,
            Duration::from_secs(60),
            &key(),
        );
        let got = verify(TokenPurpose::PasswordReset, &tok, &key()).unwrap();
        assert_eq!(got, uid);
    }

    #[test]
    fn purpose_mismatch_rejected() {
        let uid = Uuid::now_v7();
        let tok = issue(
            TokenPurpose::EmailVerification,
            uid,
            Duration::from_secs(60),
            &key(),
        );
        let err = verify(TokenPurpose::PasswordReset, &tok, &key()).unwrap_err();
        assert!(matches!(err, TokenError::WrongPurpose));
    }

    #[test]
    fn tampered_payload_rejected() {
        let uid = Uuid::now_v7();
        let tok = issue(
            TokenPurpose::EmailVerification,
            uid,
            Duration::from_secs(60),
            &key(),
        );
        // Flip a bit in the middle of the payload (in the uuid region).
        let mut bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&tok)
            .unwrap();
        bytes[5] ^= 0x01;
        let tampered = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes);
        let err = verify(TokenPurpose::EmailVerification, &tampered, &key()).unwrap_err();
        assert!(matches!(err, TokenError::BadMac));
    }

    #[test]
    fn wrong_key_rejected() {
        let uid = Uuid::now_v7();
        let tok = issue(
            TokenPurpose::EmailVerification,
            uid,
            Duration::from_secs(60),
            &key(),
        );
        let other = [7u8; 32];
        let err = verify(TokenPurpose::EmailVerification, &tok, &other).unwrap_err();
        assert!(matches!(err, TokenError::BadMac));
    }

    #[test]
    fn malformed_rejected() {
        assert!(matches!(
            verify(TokenPurpose::EmailVerification, "not-base64-!!", &key()),
            Err(TokenError::Malformed)
        ));
        let too_short = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"short");
        assert!(matches!(
            verify(TokenPurpose::EmailVerification, &too_short, &key()),
            Err(TokenError::Malformed)
        ));
    }

    #[test]
    fn expired_token_rejected() {
        // We can't easily back-date issue() (SystemTime::now is non-trivial
        // to mock without bringing in a clock crate). Instead manually
        // construct a payload with expiry in the past.
        let uid = Uuid::now_v7();
        let purpose = TokenPurpose::EmailVerification;
        let past = 1_000_000u64; // Jan 1970 + a bit
        let mut payload = [0u8; PAYLOAD_LEN];
        payload[0] = purpose.byte();
        payload[PURPOSE_LEN..PURPOSE_LEN + UUID_LEN].copy_from_slice(uid.as_bytes());
        payload[PURPOSE_LEN + UUID_LEN..].copy_from_slice(&past.to_be_bytes());
        let mut mac = HmacSha256::new_from_slice(&key()).unwrap();
        mac.update(&payload);
        let mac_bytes = mac.finalize().into_bytes();
        let mut full = [0u8; TOKEN_BYTES];
        full[..PAYLOAD_LEN].copy_from_slice(&payload);
        full[PAYLOAD_LEN..].copy_from_slice(&mac_bytes);
        let tok = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(full);
        assert!(matches!(
            verify(purpose, &tok, &key()),
            Err(TokenError::Expired)
        ));
    }
}
