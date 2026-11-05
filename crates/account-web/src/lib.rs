//! account-web: the WebAssembly module the sync server's account pages
//! load (ADR 0003). It runs the passkey ceremonies of the pages' passkey
//! forms: it shows each form, which the pages render hidden so that a
//! browser without the module offers email links alone, asks the server's
//! JSON routes for the ceremony's options, calls the browser's WebAuthn API
//! with them through webauthn-rs-proto's conversions, and returns the
//! credential. It also runs the encrypted share viewer ([`share`]), which
//! verifies, decrypts and renders a shared note in the browser.
//!
//! The module ships with `THIRD_PARTY_NOTICES.txt`, which names
//! webauthn-rs-proto under MPL-2.0 and carries the license's text;
//! `scripts/build-account-web.sh` writes both.

#![forbid(unsafe_code)]

#[cfg(target_arch = "wasm32")]
mod browser;
pub mod share;

/// A passkey form's ceremony, named by its `data-passkey` attribute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ceremony {
    /// Create a passkey to sign up with an email.
    SignUp,
    /// Sign in with a passkey of an email's account.
    SignIn,
    /// Add a passkey to the signed-in account.
    Add,
}

impl Ceremony {
    /// The ceremony a form's `data-passkey` value names.
    pub fn from_attribute(value: &str) -> Option<Self> {
        match value {
            "sign-up" => Some(Ceremony::SignUp),
            "sign-in" => Some(Ceremony::SignIn),
            "add" => Some(Ceremony::Add),
            _ => None,
        }
    }

    /// The server routes that start and finish the ceremony.
    pub fn routes(self) -> (&'static str, &'static str) {
        match self {
            Ceremony::SignUp => (
                "/account/passkeys/sign-up/start",
                "/account/passkeys/sign-up/finish",
            ),
            Ceremony::SignIn => (
                "/account/passkeys/sign-in/start",
                "/account/passkeys/sign-in/finish",
            ),
            Ceremony::Add => (
                "/account/passkeys/add/start",
                "/account/passkeys/add/finish",
            ),
        }
    }

    /// Whether the ceremony registers a new credential, rather than signing
    /// with one.
    pub fn creates(self) -> bool {
        !matches!(self, Ceremony::SignIn)
    }

    /// Whether the form names an email for the ceremony.
    pub fn takes_email(self) -> bool {
        !matches!(self, Ceremony::Add)
    }
}

/// What the page shows once the server finished a ceremony with `status`:
/// a status line, or a page to open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Show this line in the form.
    Say(&'static str),
    /// Open this page.
    Open(&'static str),
}

/// The outcome of a finished ceremony's `status`.
pub fn outcome(status: &str) -> Outcome {
    match status {
        "check_email" => Outcome::Say("Check your email for the link that confirms your address."),
        "signed_in" | "added" => Outcome::Open("/account"),
        _ => Outcome::Say("Something went wrong. Try again."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_form_names_its_routes() {
        for (value, start) in [
            ("sign-up", "/account/passkeys/sign-up/start"),
            ("sign-in", "/account/passkeys/sign-in/start"),
            ("add", "/account/passkeys/add/start"),
        ] {
            let ceremony = Ceremony::from_attribute(value).unwrap();
            assert_eq!(ceremony.routes().0, start);
            assert!(ceremony.routes().1.ends_with("/finish"));
        }
        assert_eq!(Ceremony::from_attribute("other"), None);
        assert!(Ceremony::SignUp.creates() && !Ceremony::SignIn.creates());
        assert!(!Ceremony::Add.takes_email());
        assert_eq!(outcome("signed_in"), Outcome::Open("/account"));
        assert!(matches!(outcome("check_email"), Outcome::Say(_)));
    }
}
