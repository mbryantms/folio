//! SMTP-backed transactional email (§17.1, audit M-1).
//!
//! Three implementations behind one trait so handlers don't care which is
//! active:
//!   - [`Noop`] when `COMIC_SMTP_HOST` is unset — logs the would-be email
//!     at WARN and returns Ok. Used in dev when no MTA is around.
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
}

// ───────── Noop ─────────

/// No-MTA fallback. Logs and returns Ok. The handler can still issue the
/// HMAC token and tell the user "we sent you a link" — operators in
/// development can paste the URL out of the log to complete the flow.
pub struct Noop;

#[async_trait]
impl EmailSender for Noop {
    async fn send(&self, email: Email) -> anyhow::Result<()> {
        tracing::warn!(
            to = %email.to,
            subject = %email.subject,
            "SMTP not configured — email NOT sent. Body:\n{}",
            email.body_text
        );
        Ok(())
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

    /// Drop everything in the outbox. Tests that exercise multiple flows
    /// in one TestApp use this to scope assertions.
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
        tracing::warn!("COMIC_SMTP_HOST not set — recovery emails will be logged, not sent");
        return Ok(Arc::new(Noop));
    }
    Ok(Arc::new(LettreSender::from_config(cfg)?))
}
