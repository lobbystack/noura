//! The account pages: askama templates with plain forms, no inline script.

use askama::Template;

#[derive(Template)]
#[template(path = "sign_in.html")]
pub struct SignIn;

#[derive(Template)]
#[template(path = "sign_up.html")]
pub struct SignUp;

#[derive(Template)]
#[template(path = "check_email.html")]
pub struct CheckEmail<'a> {
    pub email: &'a str,
}

/// The page a mailed link opens: one button that posts the link's token,
/// so a mail scanner that fetches the link uses nothing.
#[derive(Template)]
#[template(path = "continue.html")]
pub struct Continue<'a> {
    pub heading: &'a str,
    pub action: &'a str,
    pub token: &'a str,
    pub button: &'a str,
}

#[derive(Template)]
#[template(path = "account.html")]
pub struct Account<'a> {
    pub email: &'a str,
    pub passkeys: usize,
}

#[derive(Template)]
#[template(path = "message.html")]
pub struct Message<'a> {
    pub heading: &'a str,
    pub text: &'a str,
}
