//! XChaCha20-Poly1305 AEAD wrapper for sealing `app_setting` secret rows.
//!
//! The AEAD key lives in `secrets/settings-encryption.key` (see
//! [`crate::secrets`]). Loss of that file makes every existing secret row
//! unreadable — backup discipline is the operator's job, identical to the
//! existing pepper / jwt-ed25519 keys.
//!
//! XChaCha20 has a 192-bit (24-byte) nonce, big enough that we can safely
//! draw nonces at random per write without worrying about birthday-bound
//! collisions even if every admin in the world is rotating secrets all day.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};

const NONCE_LEN: usize = 24;

/// On-disk shape of a sealed secret. Stored as JSON in the `app_setting.value`
/// column when `is_secret = true`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SealedSecret {
    pub ciphertext: String,
    pub nonce: String,
}

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("invalid base64 in sealed secret: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("nonce has wrong length (expected 24, got {0})")]
    NonceLength(usize),
    #[error("AEAD seal/open failed")]
    Aead,
}

/// Seal a plaintext byte string under the given key. Returns the envelope
/// stored in `app_setting.value`.
pub fn seal(key: &[u8; 32], plaintext: &[u8]) -> Result<SealedSecret, CryptoError> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce: XNonce = nonce_bytes.into();
    let ct = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad: b"app_setting/v1",
            },
        )
        .map_err(|_| CryptoError::Aead)?;
    Ok(SealedSecret {
        ciphertext: STANDARD_NO_PAD.encode(ct),
        nonce: STANDARD_NO_PAD.encode(nonce_bytes),
    })
}

/// Open a sealed envelope produced by [`seal`]. Returns the plaintext.
pub fn open(key: &[u8; 32], sealed: &SealedSecret) -> Result<Vec<u8>, CryptoError> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let nonce_vec = STANDARD_NO_PAD.decode(sealed.nonce.as_bytes())?;
    let nonce_arr: [u8; NONCE_LEN] = nonce_vec
        .as_slice()
        .try_into()
        .map_err(|_| CryptoError::NonceLength(nonce_vec.len()))?;
    let nonce: XNonce = nonce_arr.into();
    let ct = STANDARD_NO_PAD.decode(sealed.ciphertext.as_bytes())?;
    cipher
        .decrypt(
            &nonce,
            Payload {
                msg: &ct,
                aad: b"app_setting/v1",
            },
        )
        .map_err(|_| CryptoError::Aead)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_then_open_roundtrips() {
        let key = [7u8; 32];
        let sealed = seal(&key, b"hunter2").expect("seal");
        let pt = open(&key, &sealed).expect("open");
        assert_eq!(pt, b"hunter2");
    }

    #[test]
    fn nonce_is_random_per_seal() {
        let key = [9u8; 32];
        let a = seal(&key, b"same").expect("seal");
        let b = seal(&key, b"same").expect("seal");
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.ciphertext, b.ciphertext);
    }

    #[test]
    fn open_rejects_wrong_key() {
        let key = [1u8; 32];
        let other = [2u8; 32];
        let sealed = seal(&key, b"secret").expect("seal");
        assert!(open(&other, &sealed).is_err());
    }

    #[test]
    fn open_rejects_tampered_ciphertext() {
        let key = [3u8; 32];
        let mut sealed = seal(&key, b"payload").expect("seal");
        // Flip one byte of base64 — still valid encoding, but the underlying
        // bytes decode to a different ciphertext and the AEAD tag rejects it.
        let mut bytes = STANDARD_NO_PAD
            .decode(sealed.ciphertext.as_bytes())
            .expect("decode");
        bytes[0] ^= 0x01;
        sealed.ciphertext = STANDARD_NO_PAD.encode(bytes);
        assert!(open(&key, &sealed).is_err());
    }
}
