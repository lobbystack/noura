//! Sending sign-in mail: SMTP through lettre, or Resend's HTTPS API, as
//! configuration picks. A transport reports only the kind of a failure:
//! providers' error texts often repeat the recipient, and the server's logs
//! hold no identifying data (§29).

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

/// The kind of a failed send, which the server logs without the
/// provider's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MailError {
    /// The server has no mail transport.
    #[error("no mail transport is configured")]
    NotConfigured,
    /// The SMTP URL doesn't parse.
    #[error("the SMTP URL doesn't parse")]
    Configuration,
    /// The sender or the recipient isn't an address the transport takes.
    #[error("an address doesn't parse")]
    Address,
    /// The transport couldn't reach the mail server, or lost it.
    #[error("the mail server can't be reached")]
    Unreachable,
    /// The SMTP server refused the message.
    #[error("the mail server refused the message")]
    Refused,
    /// Resend answered with this HTTP status.
    #[error("the mail service answered with status {0}")]
    Status(u16),
}

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
            .map_err(|_| MailError::Configuration)?
            .build();
        Ok(Self { transport })
    }
}

impl MailTransport for Smtp {
    fn send<'a>(&'a self, mail: &'a Mail) -> SendFuture<'a> {
        Box::pin(async move {
            use lettre::AsyncTransport;
            let message = lettre::Message::builder()
                .from(mail.from.parse().map_err(|_| MailError::Address)?)
                .to(mail.to.parse().map_err(|_| MailError::Address)?)
                .subject(&mail.subject)
                .header(lettre::message::header::ContentType::TEXT_PLAIN)
                .body(mail.text.clone())
                .map_err(|_| MailError::Address)?;
            self.transport.send(message).await.map(|_| ()).map_err(|e| {
                if e.is_permanent() || e.is_transient() {
                    MailError::Refused
                } else {
                    MailError::Unreachable
                }
            })
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
                .map_err(|_| MailError::Unreachable)?;
            if response.status().is_success() {
                return Ok(());
            }
            Err(MailError::Status(response.status().as_u16()))
        })
    }
}

/// No transport: every send fails, so sign-up and email sign-in can't
/// work until the operator configures mail.
pub struct NoMail;

impl MailTransport for NoMail {
    fn send<'a>(&'a self, _mail: &'a Mail) -> SendFuture<'a> {
        Box::pin(async { Err(MailError::NotConfigured) })
    }
}
