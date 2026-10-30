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

/// The device page (§7): where a signed-in person enters the user code
/// that the desktop app shows.
#[derive(Template)]
#[template(path = "device.html")]
pub struct Device<'a> {
    pub error: &'a str,
}

/// The device page before you sign in.
#[derive(Template)]
#[template(path = "device_sign_in.html")]
pub struct DeviceSignIn;

/// The device page's confirmation: the code again, with approve and deny.
#[derive(Template)]
#[template(path = "device_confirm.html")]
pub struct DeviceConfirm<'a> {
    pub email: &'a str,
    pub code: &'a str,
    pub expires: u64,
    pub confirmation: &'a str,
}

/// The share viewer (§24): account-web fetches the snapshot, verifies and
/// decrypts it with the keys in the fragment and fills the page.
#[derive(Template)]
#[template(path = "share.html")]
pub struct Share;
