//! SMTP-backed transactional email (§17.1, audit M-1).
//!
//! Three implementations behind one trait so handlers don't care which is
//! active:
//!   - [`Noop`] when `COMIC_SMTP_HOST` is unset — logs that a mail was
//!     skipped (hashed recipient + subject, never the body) at WARN and
//!     returns Ok. Used in dev when no MTA is around; debug builds also log
//!     the body at DEBUG.
//!   - [`MockSender`] for integration tests — captures into an `Arc<Mutex<…>>`
//!     so test assertions can read what was sent.
//!   - [`LettreSender`] for production — async tokio + rustls SMTP client
//!     with connection pooling.
//!
//! The trait is intentionally narrow (`send(Email) -> Result<()>`); we
//! never inspect the body or headers from the handler side.

pub mod templates;

use std::sync::Arc;

use async_trait::async_trait;
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message as LettreMessage, Tokio1Executor,
    transport::smtp::authentication::Credentials,
};
use tokio::sync::Mutex;

use crate::config::Config;

/// Last-known operational status of the outbound email sender. Surfaced by
/// `GET /admin/email/status` so an admin saving SMTP settings can confirm
/// the new wiring actually delivers without waiting for a real recovery
/// flow to fire.
#[derive(Debug, Clone, Default)]
pub struct EmailStatus {
    /// True when the active sender is not the [`Noop`] fallback. The
    /// status flips on save (via [`crate::state::AppState::replace_email`])
    /// rather than on next-send, so the UI immediately reflects whether
    /// SMTP is wired even if nothing has been sent yet.
    pub configured: bool,
    /// Wall-clock time of the last attempted send. `None` until at least
    /// one [`crate::state::AppState::send_email`] call has run.
    pub last_send_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Result of the last attempted send. `Some(true)` = success,
    /// `Some(false)` = the underlying transport returned an error,
    /// `None` = nothing has been attempted.
    pub last_send_ok: Option<bool>,
    /// Stringified transport error from the last failed send. `None` once
    /// a subsequent successful send clears the slate.
    pub last_error: Option<String>,
    pub last_duration_ms: Option<u64>,
}

impl EmailStatus {
    /// `configured` from the type of the active sender. The marker trait
    /// [`IsConfigured`] separates real senders ([`LettreSender`],
    /// [`MockSender`]) from the no-op fallback.
    pub fn from_sender(sender: &dyn EmailSender) -> Self {
        Self {
            configured: sender.is_configured(),
            ..Default::default()
        }
    }
}

/// A single transactional email. Bodies are plain-text by default; HTML is
/// optional and renders as a multipart/alternative when present.
#[derive(Clone, Debug)]
pub struct Email {
    pub to: String,
    pub subject: String,
    pub body_text: String,
    pub body_html: Option<String>,
}

#[async_trait]
pub trait EmailSender: Send + Sync {
    /// Send the email. Returns Ok even when the underlying transport
    /// is degraded if the implementation chooses to fail-open — see
    /// [`Noop`]. Production callers should treat an Err as "delivery
    /// failed but the user-visible action already succeeded" and log
    /// rather than bubble.
    async fn send(&self, email: Email) -> anyhow::Result<()>;

    /// True when this sender will actually attempt delivery. False for
    /// the [`Noop`] fallback. Drives `email_status.configured` so the
    /// admin UI can show "SMTP wired" without making a probe send.
    fn is_configured(&self) -> bool {
        true
    }
}

// ───────── Noop ─────────

/// No-MTA fallback. Logs one line that a mail was skipped and returns Ok.
/// The handler can still issue the HMAC token and tell the user "we sent
/// you a link".
///
/// The line carries the hashed recipient and the subject, never the body:
/// verification and password-reset bodies hold single-use links with their
/// tokens, and every log line also lands in the in-memory ring an admin
/// reads at `/admin/logs` (see `docs/dev/logging.md`).
pub struct Noop;

#[async_trait]
impl EmailSender for Noop {
    async fn send(&self, email: Email) -> anyhow::Result<()> {
        // Debug builds (local `cargo run`) also log the body at DEBUG so a
        // developer can follow the link without an MTA. Release builds never
        // log it: `cfg!(debug_assertions)` is false there, so no log level
        // or filter setting can bring a body into a production log.
        log_skipped(&email, cfg!(debug_assertions));
        Ok(())
    }

    fn is_configured(&self) -> bool {
        false
    }
}

/// Log that `email` was not sent. `include_body` adds a second, DEBUG-level
/// event with the plain-text body; [`Noop`] passes `cfg!(debug_assertions)`.
fn log_skipped(email: &Email, include_body: bool) {
    let email_hash = &crate::auth::cookies::sha256_hex(&email.to)[..12];
    tracing::warn!(
        email_hash = %email_hash,
        subject = %email.subject,
        "SMTP not configured — email not sent"
    );
    if include_body {
        tracing::debug!(
            email_hash = %email_hash,
            "debug build — body of the email not sent:\n{}",
            email.body_text
        );
    }
}

// ───────── Mock ─────────

/// Test sender. Stores sent emails in an `Arc<Mutex<Vec<Email>>>` so tests
/// can assert what went out without touching SMTP. Cloneable so the same
/// outbox is observable from the test code and from the handler that
/// `AppState` hands the trait object to.
#[derive(Clone, Default)]
pub struct MockSender {
    inner: Arc<Mutex<Vec<Email>>>,
}

impl MockSender {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read-only snapshot of every email sent so far.
    pub async fn outbox(&self) -> Vec<Email> {
        self.inner.lock().await.clone()
    }

    /// Most recently sent email (panics if outbox is empty — only useful
    /// in tests that just exercised a send path).
    pub async fn last(&self) -> Email {
        self.inner
            .lock()
            .await
            .last()
            .cloned()
            .expect("MockSender::last: outbox is empty")
    }

    /// Drop everything in the outbox. Tests that exercise multiple
    /// flows in one TestApp use this to scope assertions. The
    /// `#[allow(dead_code)]` is load-bearing: this is called from
    /// `tests/local_recovery.rs`, but the lib crate's dead-code lint
    /// doesn't see integration-test consumers.
    #[allow(dead_code)]
    pub async fn clear(&self) {
        self.inner.lock().await.clear();
    }
}

#[async_trait]
impl EmailSender for MockSender {
    async fn send(&self, email: Email) -> anyhow::Result<()> {
        self.inner.lock().await.push(email);
        Ok(())
    }
}

// ───────── Lettre ─────────

/// Production SMTP sender. Built with rustls + tokio under
/// `lettre`'s async API. The transport is pooled (default lettre config)
/// so multiple recovery-flow emails reuse the same TLS connection.
pub struct LettreSender {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: String,
}

impl LettreSender {
    /// Build from `Config`. Returns an error if the host is set but the
    /// transport can't be constructed (bad credentials shape, invalid
    /// port, etc.). Authentication is added only if both username and
    /// password are present.
    pub fn from_config(cfg: &Config) -> anyhow::Result<Self> {
        let host = cfg
            .smtp_host
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("smtp_host not set"))?;
        let from = cfg
            .smtp_from
            .clone()
            .ok_or_else(|| anyhow::anyhow!("smtp_from not set (required when smtp_host is)"))?;
        let mut builder = match cfg.smtp_tls.as_str() {
            "none" | "" => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host),
            "starttls" => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host)?,
            "tls" | "implicit" => AsyncSmtpTransport::<Tokio1Executor>::relay(host)?,
            other => anyhow::bail!(
                "COMIC_SMTP_TLS: unknown value `{other}` (expected: none|starttls|tls)"
            ),
        };
        builder = builder.port(cfg.smtp_port);
        if let (Some(u), Some(p)) = (
            cfg.smtp_username.as_deref().filter(|s| !s.is_empty()),
            cfg.smtp_password.as_deref().filter(|s| !s.is_empty()),
        ) {
            builder = builder.credentials(Credentials::new(u.to_owned(), p.to_owned()));
        }
        Ok(Self {
            transport: builder.build(),
            from,
        })
    }
}

#[async_trait]
impl EmailSender for LettreSender {
    async fn send(&self, email: Email) -> anyhow::Result<()> {
        let mut builder = LettreMessage::builder()
            .from(self.from.parse()?)
            .to(email.to.parse()?)
            .subject(email.subject);
        let msg = match email.body_html {
            Some(html) => builder.multipart(lettre::message::MultiPart::alternative_plain_html(
                email.body_text,
                html,
            ))?,
            None => {
                builder = builder.header(lettre::message::header::ContentType::TEXT_PLAIN);
                builder.body(email.body_text)?
            }
        };
        self.transport.send(msg).await?;
        Ok(())
    }
}

// ───────── factory ─────────

/// Decide which sender to install at boot. Production wiring lives in
/// `crate::app::serve`.
pub fn build(cfg: &Config) -> anyhow::Result<Arc<dyn EmailSender>> {
    let host_set = cfg
        .smtp_host
        .as_deref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    if !host_set {
        tracing::warn!("COMIC_SMTP_HOST not set — recovery emails will not be sent");
        return Ok(Arc::new(Noop));
    }
    Ok(Arc::new(LettreSender::from_config(cfg)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observability::{LogRingBuffer, RingLayer, SnapshotFilter};
    use tracing_subscriber::{Registry, layer::SubscriberExt};

    const TOKEN: &str = "tok-0123456789abcdef-single-use";

    /// Held by every test that reaches [`log_skipped`], so they run one at a
    /// time when plain `cargo test` runs them as threads of one process.
    ///
    /// tracing caches, for each callsite and for the whole process, whether
    /// any subscriber wants its events. The cache is filled when a thread
    /// first reaches the callsite and recomputed only when a subscriber is
    /// created. While at most one subscriber is registered, it is filled
    /// from that first thread's own default subscriber alone (tracing-core
    /// `Dispatchers::has_just_one`). Without the lock, a test thread with no
    /// subscriber could reach `log_skipped` after a capturing test had
    /// created its subscriber and before that test reached the same `warn!`
    /// / `debug!`; the cache then said "never" and the capture lost the
    /// event.
    ///
    /// Under the lock a thread with no subscriber gets there either before
    /// the capturing subscriber is created (creating it recomputes the
    /// cache) or after the capture is over. A new test that reaches
    /// `log_skipped`, for example by sending through [`Noop`], must hold it
    /// too. It is a tokio mutex because one holder is an async test.
    static LOG_SKIPPED_CALLSITES: Mutex<()> = Mutex::const_new(());

    fn capture(include_body: bool) -> Vec<crate::observability::LogEntry> {
        let _serial = LOG_SKIPPED_CALLSITES.blocking_lock();
        let ring = LogRingBuffer::with_capacity(16);
        let subscriber = Registry::default().with(RingLayer::new(ring.clone()));
        let email =
            templates::password_reset("https://comics.example.com", "reader@example.com", TOKEN);
        tracing::subscriber::with_default(subscriber, || log_skipped(&email, include_body));
        ring.snapshot(SnapshotFilter::default())
    }

    fn entry_text(e: &crate::observability::LogEntry) -> String {
        format!("{} {:?}", e.message, e.fields)
    }

    /// What a release build logs: one WARN line, no token, no link, no
    /// plain address.
    #[test]
    fn skipped_mail_is_logged_without_body() {
        let entries = capture(false);
        assert_eq!(entries.len(), 1, "{entries:?}");
        let e = &entries[0];
        assert_eq!(e.level, "warn");
        assert_eq!(e.fields["subject"], "Reset your Folio password");
        assert_eq!(
            e.fields["email_hash"],
            crate::auth::cookies::sha256_hex("reader@example.com")[..12]
        );
        let text = entry_text(e);
        assert!(!text.contains(TOKEN), "{text}");
        assert!(!text.contains("reset-password"), "{text}");
        assert!(!text.contains("reader@example.com"), "{text}");
    }

    /// Debug builds add the body, and only at DEBUG.
    #[test]
    fn debug_body_is_debug_level_only() {
        let entries = capture(true);
        assert_eq!(entries.len(), 2, "{entries:?}");
        for e in &entries {
            if entry_text(e).contains(TOKEN) {
                assert_eq!(e.level, "debug", "{e:?}");
            }
        }
        assert!(entries.iter().any(|e| entry_text(e).contains(TOKEN)));
    }

    #[tokio::test]
    async fn noop_send_succeeds_and_mock_still_captures_body() {
        let _serial = LOG_SKIPPED_CALLSITES.lock().await;
        let email =
            templates::password_reset("https://comics.example.com", "reader@example.com", TOKEN);
        assert!(Noop.send(email.clone()).await.is_ok());
        let mock = MockSender::new();
        mock.send(email).await.unwrap();
        assert!(mock.last().await.body_text.contains(TOKEN));
    }
}
