//! Plain-text + minimal-HTML email templates for the recovery flow.
//!
//! Intentionally simple — no template engine, no inline images, no
//! tracking pixels. Self-hosted Folio operators tend to run minimal
//! SMTP setups (postfix relay → providers like Resend / Postmark) and
//! the marginal complexity of a real renderer doesn't pay off when we
//! ship three templates total.
//!
//! All templates take a `public_url` (Config::public_url) and an opaque
//! `token`; the caller builds the full URL by concatenating the path.
//! The HTML version is wrapped in a very minimal block so most email
//! clients render it; nothing here is ARIA-required since the plain-
//! text is the canonical view.

use super::Email;

/// "Verify your email" — sent on register when SMTP is configured, and
/// on the resend-verification endpoint.
pub fn verify_email(public_url: &str, to: &str, token: &str) -> Email {
    let url = format!(
        "{}/auth/local/verify-email?token={}",
        public_url.trim_end_matches('/'),
        token
    );
    let body_text = format!(
        "Welcome to Folio!\n\n\
         Click the link below to verify your email address and finish setting up your account:\n\n\
         {url}\n\n\
         This link expires in 24 hours. If you didn't sign up for Folio, you can safely ignore this email."
    );
    let body_html = format!(
        "<p>Welcome to Folio!</p>\
         <p>Click the link below to verify your email address and finish setting up your account:</p>\
         <p><a href=\"{url}\">{url}</a></p>\
         <p style=\"color:#555;font-size:13px;\">This link expires in 24 hours. \
         If you didn't sign up for Folio, you can safely ignore this email.</p>"
    );
    Email {
        to: to.to_owned(),
        subject: "Verify your Folio email address".to_owned(),
        body_text,
        body_html: Some(body_html),
    }
}

/// "Reset your password" — sent by request-password-reset.
pub fn password_reset(public_url: &str, to: &str, token: &str) -> Email {
    let url = format!(
        "{}/reset-password?token={}",
        public_url.trim_end_matches('/'),
        token
    );
    let body_text = format!(
        "Someone (hopefully you) asked to reset your Folio password.\n\n\
         Open the link below to choose a new one:\n\n\
         {url}\n\n\
         This link expires in 1 hour and works only once. \
         If you didn't request a password reset, you can safely ignore this email — \
         your existing password is still in effect."
    );
    let body_html = format!(
        "<p>Someone (hopefully you) asked to reset your Folio password.</p>\
         <p>Open the link below to choose a new one:</p>\
         <p><a href=\"{url}\">{url}</a></p>\
         <p style=\"color:#555;font-size:13px;\">This link expires in 1 hour and works only once. \
         If you didn't request a password reset, you can safely ignore this email — \
         your existing password is still in effect.</p>"
    );
    Email {
        to: to.to_owned(),
        subject: "Reset your Folio password".to_owned(),
        body_text,
        body_html: Some(body_html),
    }
}

/// "Your password was changed" — confirmation after a successful reset.
/// No action link; this is informational so a compromised user notices
/// immediately and can re-secure the account.
pub fn password_changed(public_url: &str, to: &str) -> Email {
    let body_text = format!(
        "Your Folio password was just changed.\n\n\
         If you made this change, you can ignore this email.\n\n\
         If you DIDN'T change your password, sign in at {} and reset it immediately. \
         All other sessions have already been invalidated.",
        public_url.trim_end_matches('/')
    );
    let body_html = format!(
        "<p>Your Folio password was just changed.</p>\
         <p>If you made this change, you can ignore this email.</p>\
         <p>If you <strong>DIDN'T</strong> change your password, \
         <a href=\"{}\">sign in</a> and reset it immediately. \
         All other sessions have already been invalidated.</p>",
        public_url.trim_end_matches('/')
    );
    Email {
        to: to.to_owned(),
        subject: "Your Folio password was changed".to_owned(),
        body_text,
        body_html: Some(body_html),
    }
}
