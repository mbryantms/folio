//! Auth (§17.1, §17.2, §17.3, §17.6, §9.6).
//!
//! Phase 0 stages:
//!   * cookie-session JWT (access + refresh)
//!   * argon2id local users
//!   * OIDC code+PKCE (Authentik / Keycloak / Dex)
//!   * CSRF double-submit
//!   * WebSocket ticket
//!
//! These submodules are stubs in this commit so the workspace compiles. Each stub is
//! marked with a `// PHASE 0 — IMPLEMENT:` comment indicating the next concrete step.

pub mod app_password;
pub mod cookies;
pub mod csrf;
pub mod email_token;
pub mod extractor;
pub mod failed_auth;
pub mod jwt;
pub mod local;
pub mod oidc;
pub mod password;
pub mod url_signing;
pub mod ws_ticket;
pub mod xff;
// `totp` removed in M3 — stub was never implemented. The `totp_secret`
// column on `users` is left in place for forward-compat; we just don't
// read or wire it. See ~/.claude/plans/auth-hardening-1.0.md M3.

pub use extractor::{CurrentUser, RequireAdmin, RequireProgressScope};
