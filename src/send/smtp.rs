//! SMTP send via `lettre` (DESIGN.md §7).
//!
//! URL forms: `smtps://user:pass@host:465` (implicit TLS) or
//! `smtp://user:pass@host:587` (STARTTLS). Credentials are percent-decoded.
//! All TLS is rustls — no OpenSSL.

use async_trait::async_trait;
use lettre::message::{
    header::ContentType, Attachment as LettreAttachment, Mailbox, Message, MultiPart, SinglePart,
};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

use crate::diag;
use crate::error::{AppError, ErrorCode, Result};
use crate::send::{OutboundMessage, SendReceipt, Sender};
use crate::util;

/// A configured SMTP transport, reusable across sends.
pub struct SmtpSender {
    transport: AsyncSmtpTransport<Tokio1Executor>,
}

impl SmtpSender {
    /// Build the transport from a `smtp(s)://…` URL.
    pub fn from_url(url: &str) -> Result<SmtpSender> {
        let parsed = SmtpUrl::parse(url)?;
        // Log only host/port/mode — never the credentialed URL.
        diag::log(1, || {
            format!(
                "smtp transport {}:{} ({})",
                parsed.host,
                parsed.port,
                if parsed.implicit_tls {
                    "implicit-tls"
                } else {
                    "starttls"
                }
            )
        });

        let builder = if parsed.implicit_tls {
            AsyncSmtpTransport::<Tokio1Executor>::relay(&parsed.host)
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&parsed.host)
        }
        .map_err(|e| AppError::new(ErrorCode::Config, format!("smtp transport: {e}")))?;

        let mut builder = builder.port(parsed.port);
        if let Some(user) = parsed.user {
            builder = builder.credentials(Credentials::new(user, parsed.pass.unwrap_or_default()));
        }
        Ok(SmtpSender {
            transport: builder.build(),
        })
    }
}

#[async_trait]
impl Sender for SmtpSender {
    async fn send(&self, message: OutboundMessage) -> Result<SendReceipt> {
        let recipients = message.recipients();
        // We mint the Message-ID so we can report it without reading it back.
        let id = generate_message_id(&message.from);
        let email = build_email(&message, &id)?;

        diag::log(1, || {
            format!(
                "smtp send: {} recipient(s), message-id <{id}>",
                recipients.len()
            )
        });
        match self.transport.send(email).await {
            Ok(_) => {
                diag::log(1, || "smtp send: accepted".to_string());
                Ok(SendReceipt {
                    message_id: format!("<{id}>"),
                    accepted: recipients,
                    transport: "smtp",
                })
            }
            Err(e) => Err(classify_send_error(e)),
        }
    }
}

/// Assemble the lettre [`Message`] from our transport-agnostic form.
fn build_email(m: &OutboundMessage, message_id: &str) -> Result<Message> {
    let mut builder = Message::builder()
        .from(parse_mailbox("--from", &m.from)?)
        .subject(&m.subject)
        .message_id(Some(message_id.to_string()));

    for to in &m.to {
        builder = builder.to(parse_mailbox("--to", to)?);
    }
    for cc in &m.cc {
        builder = builder.cc(parse_mailbox("--cc", cc)?);
    }
    for bcc in &m.bcc {
        builder = builder.bcc(parse_mailbox("--bcc", bcc)?);
    }
    if let Some(reply_to) = &m.reply_to {
        builder = builder.reply_to(parse_mailbox("--reply-to", reply_to)?);
    }

    let body_part = if m.html {
        SinglePart::html(m.body.clone())
    } else {
        SinglePart::plain(m.body.clone())
    };

    let email = if m.attachments.is_empty() {
        builder.singlepart(body_part)
    } else {
        let mut multipart = MultiPart::mixed().singlepart(body_part);
        for att in &m.attachments {
            let content_type = ContentType::parse(&att.content_type).map_err(|e| {
                AppError::new(
                    ErrorCode::Config,
                    format!("bad content-type '{}': {e}", att.content_type),
                )
            })?;
            multipart = multipart.singlepart(
                LettreAttachment::new(att.filename.clone()).body(att.data.clone(), content_type),
            );
        }
        builder.multipart(multipart)
    }
    .map_err(|e| AppError::new(ErrorCode::Generic, format!("compose message: {e}")))?;

    Ok(email)
}

fn parse_mailbox(flag: &str, value: &str) -> Result<Mailbox> {
    value
        .parse::<Mailbox>()
        .map_err(|e| AppError::new(ErrorCode::Config, format!("{flag}: invalid address: {e}")))
}

fn generate_message_id(from: &str) -> String {
    let domain = from.rsplit('@').next().unwrap_or("tmail.local");
    format!(
        "{}{}@{}",
        util::gen_short_id(),
        util::gen_short_id(),
        domain
    )
}

/// Map a lettre SMTP error to a typed [`AppError`] (DESIGN.md §7, §36).
///
/// Permanent (5xx) failures — bad credentials or a `from` the account may not
/// use — surface as `AUTH`; everything else is treated as a transport problem.
fn classify_send_error(e: lettre::transport::smtp::Error) -> AppError {
    let code = if e.is_permanent() {
        ErrorCode::Auth
    } else {
        ErrorCode::Network
    };
    AppError::new(code, format!("smtp send failed: {e}"))
}

/// Parsed components of a `smtp(s)://` URL.
#[derive(Debug)]
struct SmtpUrl {
    implicit_tls: bool,
    host: String,
    port: u16,
    user: Option<String>,
    pass: Option<String>,
}

impl SmtpUrl {
    fn parse(url: &str) -> Result<SmtpUrl> {
        let (scheme, rest) = url
            .split_once("://")
            .ok_or_else(|| AppError::config(format!("smtp url missing scheme: '{url}'")))?;
        let implicit_tls = match scheme {
            "smtps" => true,
            "smtp" => false,
            other => {
                return Err(AppError::config(format!(
                    "smtp url scheme must be smtp or smtps, got '{other}'"
                )))
            }
        };

        // Credentials are optional; split on the LAST '@' so a ':' or '@' in
        // the (percent-encoded) password doesn't confuse host parsing.
        let (creds, host_port) = match rest.rsplit_once('@') {
            Some((creds, hp)) => (Some(creds), hp),
            None => (None, rest),
        };

        let (user, pass) = match creds {
            Some(creds) => match creds.split_once(':') {
                Some((u, p)) => (Some(pct_decode(u)), Some(pct_decode(p))),
                None => (Some(pct_decode(creds)), None),
            },
            None => (None, None),
        };

        let (host, port) = match host_port.rsplit_once(':') {
            Some((h, p)) => {
                let port = p
                    .parse::<u16>()
                    .map_err(|_| AppError::config(format!("smtp url: bad port '{p}'")))?;
                (h.to_string(), port)
            }
            None => (host_port.to_string(), if implicit_tls { 465 } else { 587 }),
        };

        if host.is_empty() {
            return Err(AppError::config("smtp url: missing host"));
        }

        Ok(SmtpUrl {
            implicit_tls,
            host,
            port,
            user,
            pass,
        })
    }
}

/// Decode `%XX` percent-escapes in a URL credential component.
fn pct_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_implicit_tls_url() {
        let u = SmtpUrl::parse("smtps://me%40gmail.com:app-pass@smtp.gmail.com:465").unwrap();
        assert!(u.implicit_tls);
        assert_eq!(u.host, "smtp.gmail.com");
        assert_eq!(u.port, 465);
        assert_eq!(u.user.as_deref(), Some("me@gmail.com")); // percent-decoded
        assert_eq!(u.pass.as_deref(), Some("app-pass"));
    }

    #[test]
    fn parses_starttls_url_and_default_port() {
        let u = SmtpUrl::parse("smtp://user:pw@mail.host").unwrap();
        assert!(!u.implicit_tls);
        assert_eq!(u.port, 587); // STARTTLS default
        assert_eq!(u.host, "mail.host");
    }

    #[test]
    fn default_port_for_smtps_is_465() {
        let u = SmtpUrl::parse("smtps://user:pw@mail.host").unwrap();
        assert_eq!(u.port, 465);
    }

    #[test]
    fn rejects_bad_scheme_and_missing_scheme() {
        assert_eq!(
            SmtpUrl::parse("http://x:1").unwrap_err().code,
            ErrorCode::Config
        );
        assert_eq!(
            SmtpUrl::parse("smtp.gmail.com:465").unwrap_err().code,
            ErrorCode::Config
        );
    }

    #[test]
    fn credentials_optional() {
        let u = SmtpUrl::parse("smtp://mail.host:2525").unwrap();
        assert!(u.user.is_none());
        assert_eq!(u.port, 2525);
    }

    #[test]
    fn build_plain_email_round_trips() {
        let msg = OutboundMessage {
            from: "me@example.com".into(),
            to: vec!["you@example.com".into()],
            cc: vec![],
            bcc: vec![],
            reply_to: None,
            subject: "hi".into(),
            body: "hello".into(),
            html: false,
            attachments: vec![],
        };
        let email = build_email(&msg, "abc@example.com").unwrap();
        // The minted Message-ID is present in the rendered headers.
        let rendered = String::from_utf8(email.formatted()).unwrap();
        assert!(rendered.contains("abc@example.com"));
    }

    #[test]
    fn bad_from_address_is_config_error() {
        let msg = OutboundMessage {
            from: "not-an-address".into(),
            to: vec!["you@example.com".into()],
            cc: vec![],
            bcc: vec![],
            reply_to: None,
            subject: "x".into(),
            body: "y".into(),
            html: false,
            attachments: vec![],
        };
        assert_eq!(
            build_email(&msg, "id@host").unwrap_err().code,
            ErrorCode::Config
        );
    }
}
