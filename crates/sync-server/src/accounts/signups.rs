//! What a server that sells plans checks at sign-up (§20): addresses at
//! disposable email domains can't sign up or sign in, and each client IP
//! starts a limited number of new accounts per UTC day. The limit counts
//! after the server answered, so the answer still reveals nothing about
//! the allowlist or which addresses hold accounts.

use std::collections::BTreeSet;
use std::time::Duration;

use sync_protocol::types::Id;
use sync_store::{Backend, Limit, Store, Taken};

/// New accounts one client IP starts per UTC day on a server that sells
/// plans, unless `NOURA_SIGN_UPS_PER_DAY` says otherwise.
pub const DEFAULT_PER_DAY: u64 = 5;

/// Widely used disposable email services. An operator adds more with
/// `NOURA_BLOCKED_EMAIL_DOMAINS`.
const DISPOSABLE: &[&str] = &[
    "10minutemail.com",
    "dispostable.com",
    "getnada.com",
    "guerrillamail.com",
    "guerrillamail.net",
    "maildrop.cc",
    "mailinator.com",
    "mailnesia.com",
    "mintemail.com",
    "mohmal.com",
    "sharklasers.com",
    "temp-mail.org",
    "tempmail.com",
    "throwawaymail.com",
    "trashmail.com",
    "yopmail.com",
];

/// The client a sign-in request came from, as the rate layer names it: a
/// keyed hash of its IP, never the address (§29).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientKey(pub Id);

/// The sign-up checks.
#[derive(Clone, Debug, Default)]
pub struct SignUpGuard {
    blocked: BTreeSet<String>,
    per_day: Option<u64>,
}

impl SignUpGuard {
    /// The built-in disposable domains plus each domain `extra` holds, one
    /// a line, and `per_day` new accounts per client IP and day.
    pub fn new(per_day: Option<u64>, extra: &str) -> Self {
        let blocked = DISPOSABLE
            .iter()
            .map(|domain| (*domain).to_owned())
            .chain(
                extra
                    .lines()
                    .map(|line| line.trim().trim_start_matches('@').to_ascii_lowercase())
                    .filter(|line| !line.is_empty() && !line.starts_with('#')),
            )
            .collect();
        Self { blocked, per_day }
    }

    /// Whether `email`, already normalized, is at a blocked domain or one
    /// of its subdomains.
    pub fn blocks(&self, email: &str) -> bool {
        let Some((_, domain)) = email.rsplit_once('@') else {
            return false;
        };
        let mut rest = domain;
        loop {
            if self.blocked.contains(rest) {
                return true;
            }
            match rest.split_once('.') {
                Some((_, parent)) if parent.contains('.') => rest = parent,
                _ => return false,
            }
        }
    }

    /// Takes one new account from `client`'s day, and returns false when
    /// the day holds no more. A request whose client the rate layer didn't
    /// name, as on a server that counts nothing, passes.
    pub async fn admit<B: Backend>(&self, store: &Store<B>, client: Option<ClientKey>) -> bool {
        let (Some(per_day), Some(ClientKey(client))) = (self.per_day, client) else {
            return true;
        };
        let limit = Limit {
            allowance: per_day,
            window: Duration::from_secs(86_400),
        };
        let key = [b"sign-up:".as_slice(), &client.0].concat();
        match store.backend().limiter().take(&key, 1, limit).await {
            Ok(Taken::Allowed) => true,
            Ok(Taken::Refused { .. }) => false,
            Err(error) => {
                tracing::error!(cause = %error, "the sign-up count couldn't be read");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disposable_domains_and_their_subdomains_are_blocked() {
        let guard = SignUpGuard::new(Some(5), "# extra\nExample-Throwaway.test\n@spam.test\n");
        assert!(guard.blocks("someone@mailinator.com"));
        assert!(guard.blocks("someone@eu.mailinator.com"));
        assert!(guard.blocks("someone@example-throwaway.test"));
        assert!(guard.blocks("someone@spam.test"));
        assert!(!guard.blocks("someone@example.com"));
        assert!(!guard.blocks("someone@notmailinator.com"));
        assert!(!guard.blocks("someone@com"));
    }
}
