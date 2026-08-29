//! Sending mail.
//!
//! Configured from the UI rather than a manifest, which forces one rule
//! throughout: **mail is never a dependency.** An invite cannot require SMTP,
//! because the person configuring SMTP is the one who would have to be invited.
//! Everything that sends also shows a link you can paste instead.
//!
//! The password is a secret, so the whole configuration lives in
//! [`CredentialStore`] rather than the plain settings table.

use std::collections::BTreeMap;
use std::sync::Arc;

use harness_persist::CredentialStore;
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use serde::Serialize;

use super::runs_routes::RunsState;

/// Credential provider the mail settings live under.
pub(crate) const PROVIDER: &str = "smtp";

/// How to reach the mail server, and who mail comes from.
#[derive(Debug, Clone)]
pub(crate) struct MailConfig {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub from: String,
    /// `starttls` (the usual 587), `tls` (implicit, 465), or `none`.
    pub encryption: String,
}

/// The same thing as the settings page shows it — never the password.
#[derive(Debug, Serialize)]
pub(crate) struct MailSummary {
    pub configured: bool,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub from: Option<String>,
    pub encryption: String,
    /// Whether a password is stored, without saying what it is.
    pub password_set: bool,
}

fn field(fields: &BTreeMap<String, String>, key: &str) -> Option<String> {
    fields
        .get(key)
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// Read the stored configuration, if it is complete enough to send with.
pub(crate) async fn config(store: &CredentialStore) -> Option<MailConfig> {
    let fields = store.get(PROVIDER).await.ok().flatten()?;
    let host = field(&fields, "host")?;
    let from = field(&fields, "from")?;
    Some(MailConfig {
        host,
        port: field(&fields, "port")
            .and_then(|p| p.parse().ok())
            .unwrap_or(587),
        username: field(&fields, "username"),
        password: field(&fields, "password"),
        from,
        encryption: field(&fields, "encryption").unwrap_or_else(|| "starttls".into()),
    })
}

/// What the settings page renders.
pub(crate) async fn summary(store: &CredentialStore) -> MailSummary {
    let fields = store.get(PROVIDER).await.ok().flatten().unwrap_or_default();
    MailSummary {
        configured: field(&fields, "host").is_some() && field(&fields, "from").is_some(),
        host: field(&fields, "host"),
        port: field(&fields, "port").and_then(|p| p.parse().ok()),
        username: field(&fields, "username"),
        from: field(&fields, "from"),
        encryption: field(&fields, "encryption").unwrap_or_else(|| "starttls".into()),
        password_set: field(&fields, "password").is_some(),
    }
}

fn transport(cfg: &MailConfig) -> Result<AsyncSmtpTransport<Tokio1Executor>, String> {
    let builder = match cfg.encryption.as_str() {
        "tls" => AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.host)
            .map_err(|e| format!("could not reach {}: {e}", cfg.host))?,
        // `none` is plain SMTP — a local relay on the same network, usually.
        "none" => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&cfg.host),
        // STARTTLS by default: the common case, and the safe one to assume.
        _ => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.host)
            .map_err(|e| format!("could not reach {}: {e}", cfg.host))?,
    };
    let builder = builder.port(cfg.port);
    let builder = match (&cfg.username, &cfg.password) {
        (Some(u), Some(p)) => builder.credentials(Credentials::new(u.clone(), p.clone())),
        // An unauthenticated relay is a normal configuration inside a cluster.
        _ => builder,
    };
    Ok(builder.build())
}

/// Send one message. `Err` carries something worth showing the operator.
pub(crate) async fn send(
    state: &Arc<RunsState>,
    to: &str,
    subject: &str,
    body: &str,
) -> Result<(), String> {
    let store = state.cred_store().await?;
    let cfg = config(store)
        .await
        .ok_or("mail is not configured — set a host and a from-address first")?;
    let message = Message::builder()
        .from(
            cfg.from
                .parse()
                .map_err(|e| format!("`{}` is not a usable from-address: {e}", cfg.from))?,
        )
        .to(to
            .parse()
            .map_err(|e| format!("`{to}` is not a usable address: {e}"))?)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_string())
        .map_err(|e| format!("could not build the message: {e}"))?;

    transport(&cfg)?
        .send(message)
        .await
        .map(|_| ())
        .map_err(|e| format!("the mail server refused it: {e}"))
}
