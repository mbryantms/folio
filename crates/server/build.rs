//! Capture build-time fingerprints (git tag + SHA + repo URL + UTC
//! timestamp) and expose them to the binary via `env!()`. Read by
//! `crate::api::server_info` for the `/admin/server` build card and by
//! `crate::api::health` for `/healthz`.
//!
//! ## Inputs (each honors a pre-set env var, then falls back to `git`):
//!
//! - `COMIC_BUILD_TAG`     — `git describe --tags --always --dirty`
//!   ("v0.1.8", "v0.1.8-3-gabcd1234", "v0.1.8-dirty", or short SHA)
//! - `COMIC_BUILD_SHA`     — short (12 char) form, for display
//! - `COMIC_BUILD_SHA_FULL`— 40-char form, for stable URL construction
//! - `COMIC_BUILD_REPO_URL`— `git config --get remote.origin.url`,
//!   normalized to `https://host/owner/repo` (strips `.git`, converts
//!   SSH → HTTPS). Same URL convention works for GitLab, Forgejo,
//!   etc. so commit + release links still resolve.
//! - `COMIC_BUILD_EPOCH`   — Unix-seconds UTC at build time, for the
//!   "Built — N hours ago" UI row.
//!
//! ## Why env-var override?
//!
//! Docker builds drop the `.git` dir from the build context, so
//! `git rev-parse` fails inside the container and would otherwise write
//! `unknown`. CI passes the COMIC_BUILD_* values as build args, which
//! the Dockerfile forwards to ENV. The build script picks up the env
//! var instead of shelling to git.
//!
//! Cargo re-runs this script when:
//!
//! - the script itself changes
//! - HEAD moves (we touch `.git/HEAD`)
//! - the current branch's ref tip moves (`.git/refs/heads/<branch>`)
//! - any of the override env vars change (CI re-tag without source change)

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // Tell cargo to re-run when HEAD or the current branch ref moves so
    // the SHA stays fresh across commits without a full clean.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    if let Ok(head) = std::fs::read_to_string("../../.git/HEAD")
        && let Some(rest) = head.trim().strip_prefix("ref: ")
    {
        println!("cargo:rerun-if-changed=../../.git/{rest}");
    }

    // CI overrides — when one of these is set in the environment the
    // build picks it up verbatim and skips the git shell-out.
    for name in [
        "COMIC_BUILD_TAG",
        "COMIC_BUILD_SHA",
        "COMIC_BUILD_SHA_FULL",
        "COMIC_BUILD_REPO_URL",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }

    let sha_full =
        env_or("COMIC_BUILD_SHA_FULL", git_sha_full).unwrap_or_else(|| "unknown".to_string());
    let sha_short = env_or("COMIC_BUILD_SHA", || {
        Some(sha_full.chars().take(12).collect::<String>()).filter(|s| s != "unknown")
    })
    .unwrap_or_else(|| "unknown".to_string());
    let tag = env_or("COMIC_BUILD_TAG", git_describe).unwrap_or_else(|| "dev".to_string());
    let repo_url = env_or("COMIC_BUILD_REPO_URL", git_repo_url).unwrap_or_default();

    println!("cargo:rustc-env=COMIC_BUILD_TAG={tag}");
    println!("cargo:rustc-env=COMIC_BUILD_SHA={sha_short}");
    println!("cargo:rustc-env=COMIC_BUILD_SHA_FULL={sha_full}");
    println!("cargo:rustc-env=COMIC_BUILD_REPO_URL={repo_url}");

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=COMIC_BUILD_EPOCH={now}");
}

/// Honor a pre-set env var first; fall back to the closure (typically a
/// `git` shell-out) if the env var is missing or empty.
fn env_or(name: &str, fallback: impl FnOnce() -> Option<String>) -> Option<String> {
    match std::env::var(name) {
        Ok(v) if !v.trim().is_empty() => Some(v),
        _ => fallback(),
    }
}

fn git_sha_full() -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// `git describe --tags --always --dirty` produces, in order of
/// preference:
///   - `v0.1.8`               — clean checkout exactly on a tag
///   - `v0.1.8-3-gabcd1234`   — 3 commits past v0.1.8
///   - `v0.1.8-dirty`         — at a tag but with uncommitted changes
///   - `abcd1234`             — no tags exist (always falls back to SHA)
fn git_describe() -> Option<String> {
    let out = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// Normalize the remote URL to an HTTPS browse URL. Inputs:
///   - `git@github.com:foo/bar.git`        → `https://github.com/foo/bar`
///   - `ssh://git@gitlab.com/foo/bar.git`  → `https://gitlab.com/foo/bar`
///   - `https://github.com/foo/bar.git`    → `https://github.com/foo/bar`
///   - `https://github.com/foo/bar`        → `https://github.com/foo/bar`
///
/// Returns `None` if there's no remote or the URL is shaped unexpectedly.
fn git_repo_url() -> Option<String> {
    let out = Command::new("git")
        .args(["config", "--get", "remote.origin.url"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8(out.stdout).ok()?;
    normalize_remote_url(raw.trim())
}

fn normalize_remote_url(raw: &str) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    let trimmed = raw.trim_end_matches(".git");

    // SCP-style: `git@host:owner/repo` (no `://`).
    if let Some(rest) = trimmed.strip_prefix("git@")
        && let Some((host, path)) = rest.split_once(':')
        && !path.is_empty()
    {
        return Some(format!("https://{host}/{path}"));
    }
    // `ssh://user@host/path` or `ssh://host/path` → strip scheme +
    // optional user prefix, prepend `https://`.
    if let Some(rest) = trimmed.strip_prefix("ssh://") {
        let after_user = rest.split_once('@').map(|(_, p)| p).unwrap_or(rest);
        return Some(format!("https://{after_user}"));
    }
    // git:// protocol (rare).
    if let Some(rest) = trimmed.strip_prefix("git://") {
        return Some(format!("https://{rest}"));
    }
    // Already HTTPS / HTTP.
    if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
        return Some(trimmed.to_owned());
    }
    // Unknown shape — surface as `None` rather than misleading the UI
    // with an unclickable string.
    None
}

// build.rs unit tests would need a separate testable module since
// `cargo test` doesn't compile build scripts as test targets. The
// repo-URL shape contract is end-to-end tested via the integration
// test in `tests/admin_stats.rs::server_info_carries_build_fingerprint_fields`
// which asserts the returned `repo_url` is HTTP(S)-shaped when present.
