//! Server-side secrets stored under `/data/secrets/` (§17.9).
//!
//! Secrets are loaded if present, generated and persisted otherwise.
//! Files are written with mode `0600`. The directory itself is `0700`.
//!
//! Files:
//!   `pepper`            — 32 bytes, argon2id pepper.
//!   `jwt-ed25519.key`   — 32-byte Ed25519 private key (raw).
//!   `csrf.key`          — 32 bytes, HMAC-SHA256 secret for stateless CSRF in some flows
//!                         (currently the double-submit pattern uses cookie value comparison
//!                         and doesn't need this; reserved for future use).
//!   `email-token.key`   — 32 bytes, HMAC-SHA256 for stateless email verification/reset tokens.
//!   `url-signing.key`   — 32 bytes, HMAC-SHA256 for OPDS-PSE signed URLs.
//!
//! All loaders are idempotent. Workflow:
//!   `Secrets::load(&data_dir)?`

use ed25519_dalek::{SECRET_KEY_LENGTH, SigningKey};
use rand::RngCore;
use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

#[derive(Clone)]
pub struct Secrets {
    pub pepper: Zeroizing<[u8; 32]>,
    pub jwt_ed25519: SigningKey,
    pub email_token_key: Zeroizing<[u8; 32]>,
    pub url_signing_key: Zeroizing<[u8; 32]>,
}

impl Secrets {
    pub fn load(data_dir: &Path) -> anyhow::Result<Self> {
        let dir = data_dir.join("secrets");
        ensure_dir(&dir)?;

        let pepper = load_or_generate_bytes::<32>(&dir.join("pepper"))?;
        let jwt = load_or_generate_ed25519(&dir.join("jwt-ed25519.key"))?;
        let email = load_or_generate_bytes::<32>(&dir.join("email-token.key"))?;
        let url = load_or_generate_bytes::<32>(&dir.join("url-signing.key"))?;

        Ok(Self {
            pepper: Zeroizing::new(pepper),
            jwt_ed25519: jwt,
            email_token_key: Zeroizing::new(email),
            url_signing_key: Zeroizing::new(url),
        })
    }
}

fn ensure_dir(dir: &Path) -> anyhow::Result<()> {
    if !dir.exists() {
        fs::create_dir_all(dir)?;
    }
    #[cfg(unix)]
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn load_or_generate_bytes<const N: usize>(path: &PathBuf) -> anyhow::Result<[u8; N]> {
    if path.exists() {
        let bytes = fs::read(path)?;
        if bytes.len() != N {
            anyhow::bail!(
                "secret file {} has wrong length (got {}, expected {})",
                path.display(),
                bytes.len(),
                N
            );
        }
        let mut out = [0u8; N];
        out.copy_from_slice(&bytes);
        Ok(out)
    } else {
        let mut out = [0u8; N];
        rand::thread_rng().fill_bytes(&mut out);
        write_secret(path, &out)?;
        tracing::info!(path = %path.display(), "generated new {}-byte secret", N);
        Ok(out)
    }
}

fn load_or_generate_ed25519(path: &PathBuf) -> anyhow::Result<SigningKey> {
    if path.exists() {
        let bytes = fs::read(path)?;
        if bytes.len() != SECRET_KEY_LENGTH {
            anyhow::bail!(
                "ed25519 key file {} has wrong length (got {}, expected {})",
                path.display(),
                bytes.len(),
                SECRET_KEY_LENGTH
            );
        }
        let mut secret = [0u8; SECRET_KEY_LENGTH];
        secret.copy_from_slice(&bytes);
        Ok(SigningKey::from_bytes(&secret))
    } else {
        let mut secret = [0u8; SECRET_KEY_LENGTH];
        rand::thread_rng().fill_bytes(&mut secret);
        let key = SigningKey::from_bytes(&secret);
        write_secret(path, &secret)?;
        tracing::info!(path = %path.display(), "generated new Ed25519 keypair");
        Ok(key)
    }
}

fn write_secret(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.create_new(true).write(true);
    #[cfg(unix)]
    opts.mode(0o600);
    let mut f = opts.open(path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}
