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
    /// Whether the server sells plans, which shows the plan section.
    pub billing: bool,
    pub plan_id: &'a str,
    pub plan_name: &'a str,
    pub usage: &'a str,
    pub past_due: bool,
    /// The line about workspaces over plan, or empty.
    pub over_plan: &'a str,
    /// The plans to upgrade to: plan ID and name.
    pub offers: &'a [(&'static str, &'static str)],
    pub manage: bool,
}

/// The step between a link from elsewhere and the checkout: a refresh from
/// this page, a navigation within the server's own site, sends the web
/// session's strict cookie that the link's navigation didn't.
#[derive(Template)]
#[template(path = "upgrade.html")]
pub struct Upgrade<'a> {
    pub plan: &'a str,
    pub name: &'a str,
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

/// The invitation page (§11): it tells a browser visitor to open the link
/// in Noura, and loads no script, so nothing on it reads the fragment that
/// holds the invitation's secret.
#[derive(Template)]
#[template(path = "invite.html")]
pub struct Invite<'a> {
    pub heading: &'a str,
    pub text: &'a str,
}

/// The share viewer (§24): account-web fetches the snapshot, verifies and
/// decrypts it with the keys in the fragment and fills the page.
#[derive(Template)]
#[template(path = "share.html")]
pub struct Share;
