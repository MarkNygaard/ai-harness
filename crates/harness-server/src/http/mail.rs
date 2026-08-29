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
            .map_err(|e| format!("could not reach {}: {}", cfg.host, error_chain(&e)))?,
        // `none` is plain SMTP — a local relay on the same network, usually.
        "none" => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&cfg.host),
        // STARTTLS by default: the common case, and the safe one to assume.
        _ => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.host)
            .map_err(|e| format!("could not reach {}: {}", cfg.host, error_chain(&e)))?,
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

    // What was attempted, so a failure in the log can be matched to a
    // configuration without the operator having to guess which one was live.
    // Everything here is non-secret: the password is deliberately not a field.
    tracing::info!(
        host = %cfg.host,
        port = cfg.port,
        encryption = %cfg.encryption,
        username = cfg.username.as_deref().unwrap_or("<none>"),
        auth = cfg.password.is_some(),
        from = %cfg.from,
        to = %to,
        "smtp: sending"
    );

    match transport(&cfg)?.send(message).await {
        Ok(_) => {
            tracing::info!(host = %cfg.host, to = %to, "smtp: accepted");
            Ok(())
        }
        Err(e) => {
            let detail = error_chain(&e);
            tracing::warn!(
                host = %cfg.host,
                port = cfg.port,
                encryption = %cfg.encryption,
                auth = cfg.password.is_some(),
                error = %detail,
                "smtp: send failed"
            );
            Err(format!("the mail server refused it: {detail}"))
        }
    }
}

/// An error and everything underneath it, joined.
///
/// lettre's top-level `Display` is usually a category -- "Connection error" --
/// and the sentence that identifies the problem ("certificate verify failed",
/// "invalid response: 535 auth failed") lives one or two sources down. Showing
/// only the top is the difference between a fixable report and a shrug, and
/// this string is what both the log and the settings page get.
fn error_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut parts = vec![err.to_string()];
    let mut source = err.source();
    while let Some(e) = source {
        let text = e.to_string();
        // Skip a source that only repeats its parent.
        if !parts.iter().any(|p| p == &text) {
            parts.push(text);
        }
        source = e.source();
    }
    parts.join(": ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt;

    #[derive(Debug)]
    struct Layer(&'static str, Option<Box<Layer>>);

    impl fmt::Display for Layer {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.0)
        }
    }

    impl std::error::Error for Layer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.1
                .as_deref()
                .map(|e| e as &(dyn std::error::Error + 'static))
        }
    }

    #[test]
    fn keeps_the_sentence_that_identifies_the_problem() {
        // The shape lettre actually produces: a useless category on top, the
        // real cause underneath.
        let err = Layer(
            "Connection error",
            Some(Box::new(Layer("certificate verify failed", None))),
        );
        assert_eq!(
            error_chain(&err),
            "Connection error: certificate verify failed"
        );
    }

    #[test]
    fn does_not_repeat_a_source_that_echoes_its_parent() {
        let err = Layer("timed out", Some(Box::new(Layer("timed out", None))));
        assert_eq!(error_chain(&err), "timed out");
    }

    #[test]
    fn a_lone_error_is_just_itself() {
        assert_eq!(error_chain(&Layer("nope", None)), "nope");
    }
}
