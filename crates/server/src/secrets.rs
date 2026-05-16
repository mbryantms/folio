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
    /// Per-call metadata so the caller in `app::serve` can refuse to
    /// boot when a freshly-generated pepper / settings key would
    /// silently invalidate existing DB rows (the prod-incident class
    /// that locked everyone out for an afternoon).
    pub load_report: LoadReport,
}

impl Secrets {
    pub fn load(data_dir: &Path) -> anyhow::Result<Self> {
        let dir = data_dir.join("secrets");
        let dir_was_fresh = ensure_dir(&dir)?;

        let mut report = LoadReport {
            dir_was_fresh,
            ..Default::default()
        };
        let pepper_path = dir.join("pepper");
        let pepper_existed = pepper_path.exists();
        let pepper = load_or_generate_bytes::<32>(&pepper_path, &mut report)?;
        if !pepper_existed {
            report.pepper_regenerated = true;
        }
        let jwt = load_or_generate_ed25519(&dir.join("jwt-ed25519.key"), &mut report)?;
        let email = load_or_generate_bytes::<32>(&dir.join("email-token.key"), &mut report)?;
        let url = load_or_generate_bytes::<32>(&dir.join("url-signing.key"), &mut report)?;
        let settings_path = dir.join("settings-encryption.key");
        let settings_existed = settings_path.exists();
        let settings = load_or_generate_bytes::<32>(&settings_path, &mut report)?;
        if !settings_existed {
            report.settings_key_regenerated = true;
        }

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
            load_report: report,
        })
    }
}

/// Per-load counter so [`Secrets::load`] can emit a single summary line
/// instead of one INFO per file, and decide whether the regenerated
/// counts represent first-boot setup or post-boot data loss. The
/// `pepper_regenerated` / `settings_key_regenerated` flags let the
/// caller in `app::serve` cross-check against DB state — a freshly-
/// generated pepper on a database with existing password hashes means
/// every user is locked out, which we refuse to boot through silently.
#[derive(Clone, Debug, Default)]
pub struct LoadReport {
    pub loaded: usize,
    pub regenerated: usize,
    pub dir_was_fresh: bool,
    pub pepper_regenerated: bool,
    pub settings_key_regenerated: bool,
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

#[cfg(test)]
mod tests {
    //! `load_report` carries the per-secret regeneration flags that
    //! `app::serve` uses to refuse boot when a regenerated pepper would
    //! brick existing password hashes. These tests pin the two
    //! transitions: fresh-dir vs. partial-load (someone deleted only
    //! the pepper file).
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn fresh_dir_regenerates_both_pepper_and_settings_key() {
        let tmp = TempDir::new().unwrap();
        let secrets = Secrets::load(tmp.path()).unwrap();
        assert!(secrets.load_report.dir_was_fresh);
        assert!(secrets.load_report.pepper_regenerated);
        assert!(secrets.load_report.settings_key_regenerated);
        assert_eq!(secrets.load_report.regenerated, 5);
        assert_eq!(secrets.load_report.loaded, 0);
    }

    #[test]
    fn second_load_loads_existing_secrets() {
        let tmp = TempDir::new().unwrap();
        let first = Secrets::load(tmp.path()).unwrap();
        let second = Secrets::load(tmp.path()).unwrap();
        assert!(!second.load_report.dir_was_fresh);
        assert!(!second.load_report.pepper_regenerated);
        assert!(!second.load_report.settings_key_regenerated);
        assert_eq!(second.load_report.regenerated, 0);
        assert_eq!(second.load_report.loaded, 5);
        // Same bytes both times.
        assert_eq!(&first.pepper[..], &second.pepper[..]);
        assert_eq!(
            &first.settings_encryption_key[..],
            &second.settings_encryption_key[..]
        );
    }

    #[test]
    fn deleting_only_pepper_sets_pepper_regenerated_flag() {
        // The catastrophic mid-life case: dir exists, other secrets
        // load fine, but `pepper` got removed somehow. We want the
        // flag set so `app::serve` can refuse boot if password hashes
        // exist.
        let tmp = TempDir::new().unwrap();
        Secrets::load(tmp.path()).unwrap();
        fs::remove_file(tmp.path().join("secrets/pepper")).unwrap();
        let again = Secrets::load(tmp.path()).unwrap();
        assert!(!again.load_report.dir_was_fresh);
        assert!(again.load_report.pepper_regenerated);
        assert!(!again.load_report.settings_key_regenerated);
        assert_eq!(again.load_report.regenerated, 1);
        assert_eq!(again.load_report.loaded, 4);
    }

    #[test]
    fn deleting_only_settings_key_sets_settings_key_regenerated_flag() {
        let tmp = TempDir::new().unwrap();
        Secrets::load(tmp.path()).unwrap();
        fs::remove_file(tmp.path().join("secrets/settings-encryption.key")).unwrap();
        let again = Secrets::load(tmp.path()).unwrap();
        assert!(!again.load_report.pepper_regenerated);
        assert!(again.load_report.settings_key_regenerated);
    }
}
