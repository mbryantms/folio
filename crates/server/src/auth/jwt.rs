//! JWT issuance / verification with `users.token_version` revocation (§17.2).
//!
//! Algorithm: EdDSA (Ed25519). Server holds the keypair under
//! `/data/secrets/jwt-ed25519.key` (loaded by [`crate::secrets`]).
//!
//! When OIDC is the auth source, we still mint our **own** access JWT after the
//! upstream login — we never re-use the issuer's ID token as a session credential.
//! That keeps `token_version` revocation effective regardless of issuer.

use chrono::{Duration, Utc};
use ed25519_dalek::SigningKey;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessClaims {
    pub iss: String,
    pub sub: String, // user id
    pub aud: String,
    pub exp: i64,
    pub iat: i64,
    pub tv: i64, // token_version snapshot
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshClaims {
    pub iss: String,
    pub sub: String,
    pub jti: String, // session id (auth_sessions.id)
    pub exp: i64,
    pub iat: i64,
}

/// Per-process JWT signer/verifier built from the Ed25519 keypair.
pub struct JwtKeys {
    enc: EncodingKey,
    dec: DecodingKey,
    issuer: String,
    audience: String,
}

const AUDIENCE: &str = "comic-reader";

impl JwtKeys {
    pub fn from_secret(signing: &SigningKey, public_url: &str) -> anyhow::Result<Self> {
        // jsonwebtoken 9 (backed by ed25519-compact):
        //   EncodingKey::from_ed_der wants PKCS#8 (v1 or v2) of the private key.
        //   DecodingKey::from_ed_der wants the RAW 32-byte public key.
        let priv_pkcs8 = ed25519_to_pkcs8_v2(signing.to_bytes());
        let pub_raw = signing.verifying_key().to_bytes();
        Ok(Self {
            enc: EncodingKey::from_ed_der(&priv_pkcs8),
            dec: DecodingKey::from_ed_der(&pub_raw),
            issuer: public_url.trim_end_matches('/').to_string(),
            audience: AUDIENCE.to_string(),
        })
    }

    pub fn issue_access(
        &self,
        user_id: Uuid,
        role: &str,
        token_version: i64,
        ttl: Duration,
    ) -> anyhow::Result<String> {
        let now = Utc::now();
        let claims = AccessClaims {
            iss: self.issuer.clone(),
            sub: user_id.to_string(),
            aud: self.audience.clone(),
            exp: (now + ttl).timestamp(),
            iat: now.timestamp(),
            tv: token_version,
            role: role.to_string(),
        };
        Ok(encode(&Header::new(Algorithm::EdDSA), &claims, &self.enc)?)
    }

    pub fn issue_refresh(
        &self,
        user_id: Uuid,
        session_id: Uuid,
        ttl: Duration,
    ) -> anyhow::Result<String> {
        let now = Utc::now();
        let claims = RefreshClaims {
            iss: self.issuer.clone(),
            sub: user_id.to_string(),
            jti: session_id.to_string(),
            exp: (now + ttl).timestamp(),
            iat: now.timestamp(),
        };
        Ok(encode(&Header::new(Algorithm::EdDSA), &claims, &self.enc)?)
    }

    pub fn verify_access(&self, token: &str) -> anyhow::Result<AccessClaims> {
        let mut v = Validation::new(Algorithm::EdDSA);
        v.set_audience(&[&self.audience]);
        v.set_issuer(&[&self.issuer]);
        v.leeway = 60;
        Ok(decode::<AccessClaims>(token, &self.dec, &v)?.claims)
    }

    pub fn verify_refresh(&self, token: &str) -> anyhow::Result<RefreshClaims> {
        let mut v = Validation::new(Algorithm::EdDSA);
        v.validate_aud = false; // refresh tokens don't carry aud
        v.set_issuer(&[&self.issuer]);
        v.leeway = 60;
        Ok(decode::<RefreshClaims>(token, &self.dec, &v)?.claims)
    }
}

// ────────────────────────────────────────────────────────────────────────
// PKCS#8 v2 / SPKI wrappers for Ed25519 raw keys.
//
// jsonwebtoken's from_ed_der wants PKCS#8 v2 (with public key) for the private
// side and SPKI for the public side. ed25519-dalek can produce these via
// pkcs8 traits when the `pkcs8` feature is enabled.
// ────────────────────────────────────────────────────────────────────────

fn ed25519_to_pkcs8_v2(secret: [u8; 32]) -> Vec<u8> {
    use ed25519_dalek::pkcs8::EncodePrivateKey;
    let key = SigningKey::from_bytes(&secret);
    key.to_pkcs8_der()
        .expect("encode pkcs8")
        .as_bytes()
        .to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::RngCore;

    fn keys() -> JwtKeys {
        let mut secret = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut secret);
        let signing = SigningKey::from_bytes(&secret);
        JwtKeys::from_secret(&signing, "https://example.com").unwrap()
    }

    #[test]
    fn access_round_trip() {
        let k = keys();
        let uid = Uuid::now_v7();
        let tok = k
            .issue_access(uid, "user", 7, Duration::minutes(15))
            .unwrap();
        let claims = k.verify_access(&tok).unwrap();
        assert_eq!(claims.sub, uid.to_string());
        assert_eq!(claims.tv, 7);
        assert_eq!(claims.role, "user");
    }

    #[test]
    fn refresh_round_trip() {
        let k = keys();
        let uid = Uuid::now_v7();
        let sid = Uuid::now_v7();
        let tok = k.issue_refresh(uid, sid, Duration::days(30)).unwrap();
        let claims = k.verify_refresh(&tok).unwrap();
        assert_eq!(claims.sub, uid.to_string());
        assert_eq!(claims.jti, sid.to_string());
    }
}
