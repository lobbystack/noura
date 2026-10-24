//! Sending sign-in mail: SMTP through lettre, or Resend's HTTPS API, as
//! configuration picks. A transport reports failure without the provider's
//! detail, which stays in the server's local log.

use std::future::Future;
use std::pin::Pin;

/// One message to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mail {
    /// The sender.
    pub from: String,
    /// The recipient.
    pub to: String,
    /// The subject.
    pub subject: String,
    /// The plain-text body.
    pub text: String,
}

/// A failed send. Its text belongs in the local log.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct MailError(pub String);

/// A boxed send future.
pub type SendFuture<'a> = Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>>;

/// A way to send mail.
pub trait MailTransport: Send + Sync {
    /// Sends one message.
    fn send<'a>(&'a self, mail: &'a Mail) -> SendFuture<'a>;
}

/// SMTP through lettre, configured by a URL such as
/// `smtps://user:password@smtp.example.com` or
/// `smtp://smtp.example.com:587?tls=required`.
pub struct Smtp {
    transport: lettre::AsyncSmtpTransport<lettre::Tokio1Executor>,
}

impl Smtp {
    /// A transport for `url`.
    pub fn from_url(url: &str) -> Result<Self, MailError> {
        let transport = lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::from_url(url)
            .map_err(|e| MailError(format!("the SMTP URL doesn't parse: {e}")))?
            .build();
        Ok(Self { transport })
    }
}

impl MailTransport for Smtp {
    fn send<'a>(&'a self, mail: &'a Mail) -> SendFuture<'a> {
        Box::pin(async move {
            use lettre::AsyncTransport;
            let message = lettre::Message::builder()
                .from(
                    mail.from
                        .parse()
                        .map_err(|e| MailError(format!("the sender doesn't parse: {e}")))?,
                )
                .to(mail
                    .to
                    .parse()
                    .map_err(|e| MailError(format!("the recipient doesn't parse: {e}")))?)
                .subject(&mail.subject)
                .header(lettre::message::header::ContentType::TEXT_PLAIN)
                .body(mail.text.clone())
                .map_err(|e| MailError(format!("the message doesn't build: {e}")))?;
            self.transport
                .send(message)
                .await
                .map(|_| ())
                .map_err(|e| MailError(format!("SMTP delivery failed: {e}")))
        })
    }
}

/// Resend's HTTPS API, for hosts that block SMTP's ports.
pub struct Resend {
    http: reqwest::Client,
    api_key: String,
    endpoint: String,
}

/// Resend's endpoint for sending one message.
pub const RESEND_ENDPOINT: &str = "https://api.resend.com/emails";

impl Resend {
    /// A transport with `api_key`, posting to `endpoint`.
    pub fn new(api_key: String, endpoint: Option<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_key,
            endpoint: endpoint.unwrap_or_else(|| RESEND_ENDPOINT.to_owned()),
        }
    }
}

impl MailTransport for Resend {
    fn send<'a>(&'a self, mail: &'a Mail) -> SendFuture<'a> {
        Box::pin(async move {
            let body = serde_json::json!({
                "from": mail.from,
                "to": [mail.to],
                "subject": mail.subject,
                "text": mail.text,
            });
            let response = self
                .http
                .post(&self.endpoint)
                .bearer_auth(&self.api_key)
                .header("content-type", "application/json")
                .body(body.to_string())
                .send()
                .await
                .map_err(|e| MailError(format!("Resend can't be reached: {e}")))?;
            if response.status().is_success() {
                return Ok(());
            }
            let status = response.status();
            let detail: String = response
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(500)
                .collect();
            Err(MailError(format!(
                "Resend refused the message ({status}): {detail}"
            )))
        })
    }
}

/// No transport: every send fails, so sign-up and email sign-in can't
/// work until the operator configures mail.
pub struct NoMail;

impl MailTransport for NoMail {
    fn send<'a>(&'a self, _mail: &'a Mail) -> SendFuture<'a> {
        Box::pin(async { Err(MailError("no mail transport is configured".into())) })
    }
}
