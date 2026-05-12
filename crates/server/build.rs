//! Capture build-time fingerprints (git SHA + UTC timestamp) and expose them
//! to the binary via `env!()`. Read by `crate::api::health` so `/healthz`
//! reports which build is actually running — `just dev-status` relies on
//! this to flag stale servers.
//!
//! Cargo re-runs this script when:
//!   - the script itself changes
//!   - HEAD moves (we touch `.git/HEAD` via `cargo:rerun-if-changed`)
//!   - the current branch's ref tip moves (`.git/refs/heads/<branch>`)
//!
//! No external crate dependency — `git` from PATH + `chrono` not needed
//! (we use a UTC timestamp via `SystemTime` formatted by hand).

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // Tell cargo to re-run when HEAD or the current branch ref moves so the
    // SHA stays fresh across commits without a full clean.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    if let Ok(head) = std::fs::read_to_string("../../.git/HEAD")
        && let Some(rest) = head.trim().strip_prefix("ref: ")
    {
        println!("cargo:rerun-if-changed=../../.git/{rest}");
    }

    let sha = git_sha().unwrap_or_else(|| "unknown".to_string());
    let dirty = git_is_dirty().unwrap_or(false);
    let stamp = if dirty { format!("{sha}-dirty") } else { sha };
    println!("cargo:rustc-env=COMIC_BUILD_SHA={stamp}");

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=COMIC_BUILD_EPOCH={now}");
}

fn git_sha() -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
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

fn git_is_dirty() -> Option<bool> {
    let out = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(!out.stdout.is_empty())
}
