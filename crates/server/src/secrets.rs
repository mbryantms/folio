//! Server-side secrets stored under `/data/secrets/` (§17.9).
//!
//! Secrets are loaded if present, generated and persisted otherwise.
//! Files are written with mode `0600`. The directory itself is `0700`.
//!
//! Files:
//!   `pepper`                    — 32 bytes, argon2id pepper.
//!   `jwt-ed25519.key`           — 32-byte Ed25519 private key (raw).
//!   `csrf.key`                  — 32 bytes, HMAC-SHA256 secret for stateless CSRF in some flows
//!                                 (currently the double-submit pattern uses cookie value comparison
//!                                 and doesn't need this; reserved for future use).
//!   `email-token.key`           — 32 bytes, HMAC-SHA256 for stateless email verification/reset tokens.
//!   `url-signing.key`           — 32 bytes, HMAC-SHA256 for OPDS-PSE signed URLs.
//!   `settings-encryption.key`   — 32 bytes, XChaCha20-Poly1305 AEAD key for
//!                                 sealing secret rows in the `app_setting`
//!                                 table (SMTP password, OIDC client secret).
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
    pub settings_encryption_key: Zeroizing<[u8; 32]>,
}

impl Secrets {
    pub fn load(data_dir: &Path) -> anyhow::Result<Self> {
        let dir = data_dir.join("secrets");
        let dir_was_fresh = ensure_dir(&dir)?;

        let mut report = LoadReport {
            dir_was_fresh,
            ..Default::default()
        };
        let pepper = load_or_generate_bytes::<32>(&dir.join("pepper"), &mut report)?;
        let jwt = load_or_generate_ed25519(&dir.join("jwt-ed25519.key"), &mut report)?;
        let email = load_or_generate_bytes::<32>(&dir.join("email-token.key"), &mut report)?;
        let url = load_or_generate_bytes::<32>(&dir.join("url-signing.key"), &mut report)?;
        let settings =
            load_or_generate_bytes::<32>(&dir.join("settings-encryption.key"), &mut report)?;

        // Loud boot diagnostic: if any secret had to be regenerated and the
        // directory was *not* freshly created, the volume mount probably
        // lost data between deploys. Existing local-auth users will see
        // "wrong password" until they reset, because the pepper is mixed
        // into every argon2 hash. See docs/install/secrets-backup.md.
        if report.regenerated > 0 && !report.dir_was_fresh {
            tracing::error!(
                regenerated = report.regenerated,
                loaded = report.loaded,
                dir = %dir.display(),
                "SECRETS REGENERATED ON A NON-EMPTY DATA DIR. \
                 Every existing local password hash now fails verification because \
                 the argon2 pepper was rewritten. This usually means the docker \
                 volume mount lost data (e.g. `docker compose down -v` between \
                 pulls, or the compose project name changed). Recovery: restore \
                 /data/secrets from backup, OR have affected users go through \
                 /forgot-password. See docs/install/secrets-backup.md."
            );
        } else {
            tracing::info!(
                regenerated = report.regenerated,
                loaded = report.loaded,
                dir = %dir.display(),
                fresh_dir = report.dir_was_fresh,
                "secrets loaded"
            );
        }

        Ok(Self {
            pepper: Zeroizing::new(pepper),
            jwt_ed25519: jwt,
            email_token_key: Zeroizing::new(email),
            url_signing_key: Zeroizing::new(url),
            settings_encryption_key: Zeroizing::new(settings),
        })
    }
}

/// Per-load counter so [`Secrets::load`] can emit a single summary line
/// instead of one INFO per file, and decide whether the regenerated
/// counts represent first-boot setup or post-boot data loss.
#[derive(Default)]
struct LoadReport {
    loaded: usize,
    regenerated: usize,
    dir_was_fresh: bool,
}

/// Returns `true` when this call created the directory (i.e. it didn't
/// exist on entry). Used by [`Secrets::load`] to distinguish first-boot
/// setup ("fresh dir, everything generated, normal") from post-boot data
/// loss ("dir existed, but pepper was missing — operator should
/// investigate").
fn ensure_dir(dir: &Path) -> anyhow::Result<bool> {
    let created = !dir.exists();
    if created {
        fs::create_dir_all(dir)?;
    }
    #[cfg(unix)]
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    Ok(created)
}

fn load_or_generate_bytes<const N: usize>(
    path: &PathBuf,
    report: &mut LoadReport,
) -> anyhow::Result<[u8; N]> {
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
        report.loaded += 1;
        Ok(out)
    } else {
        let mut out = [0u8; N];
        rand::thread_rng().fill_bytes(&mut out);
        write_secret(path, &out)?;
        report.regenerated += 1;
        Ok(out)
    }
}

fn load_or_generate_ed25519(path: &PathBuf, report: &mut LoadReport) -> anyhow::Result<SigningKey> {
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
        report.loaded += 1;
        Ok(SigningKey::from_bytes(&secret))
    } else {
        let mut secret = [0u8; SECRET_KEY_LENGTH];
        rand::thread_rng().fill_bytes(&mut secret);
        let key = SigningKey::from_bytes(&secret);
        write_secret(path, &secret)?;
        report.regenerated += 1;
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
