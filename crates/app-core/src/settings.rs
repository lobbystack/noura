//! Settings: its sections, grouped under This computer, Workspace and App,
//! and the view state of the open Settings window. The kernel's sections
//! come first; later areas such as plugins, AI and MCP add their own with
//! [`App::register_settings_section`](crate::App::register_settings_section).

use crate::{Notice, OpenAtLogin};

/// The groups Settings lists its sections under, in the order it shows
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SettingsGroup {
    /// Choices that stay on this device, such as the theme.
    ThisComputer,
    /// Choices stored in the open workspace.
    Workspace,
    /// The app itself, such as its version.
    App,
}

impl SettingsGroup {
    /// Every group, in the order Settings shows them.
    pub const ALL: [Self; 3] = [Self::ThisComputer, Self::Workspace, Self::App];

    pub fn title(self) -> &'static str {
        match self {
            Self::ThisComputer => "This computer",
            Self::Workspace => "Workspace",
            Self::App => "App",
        }
    }
}

/// The IDs of the kernel's sections.
pub mod settings_section {
    /// Appearance, zoom and Open at login.
    pub const GENERAL: &str = "general";
    /// Text size.
    pub const EDITOR: &str = "editor";
    /// Every shortcut by area.
    pub const SHORTCUTS: &str = "shortcuts";
    /// The workspace's name and folder.
    pub const WORKSPACE: &str = "workspace";
    /// The plugins installed on this device and enabled in the workspace.
    pub const PLUGINS: &str = "plugins";
    /// Whether the open workspace syncs, and how its sync stands.
    pub const SYNC: &str = "workspace-sync";
    /// The sync account: the server, this computer's sign-in, your devices
    /// and the recovery kit.
    pub const SYNC_ACCOUNT: &str = "sync-account";
    /// The AI providers on this device, their keys, and your consent for
    /// the open workspace.
    pub const AI: &str = "ai-providers";
    /// The version, license, links and Copy diagnostics.
    pub const ABOUT: &str = "about";
    /// Check for updates, and where updating stands.
    pub const UPDATES: &str = "updates";
    /// The MCP configuration that connects AI tools outside Noura, which
    /// shows once a client offers the MCP server.
    pub const AI_TOOLS: &str = "ai-tools";
}

/// One section of Settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsSection {
    pub id: String,
    pub title: String,
    pub group: SettingsGroup,
}

/// The sections every Noura has, in the order they register.
pub(crate) fn kernel_sections() -> Vec<SettingsSection> {
    [
        (
            settings_section::GENERAL,
            "General",
            SettingsGroup::ThisComputer,
        ),
        (
            settings_section::EDITOR,
            "Editor",
            SettingsGroup::ThisComputer,
        ),
        (
            settings_section::SHORTCUTS,
            "Keyboard shortcuts",
            SettingsGroup::ThisComputer,
        ),
        (
            settings_section::SYNC_ACCOUNT,
            "Sync account",
            SettingsGroup::ThisComputer,
        ),
        (
            settings_section::AI,
            "AI providers",
            SettingsGroup::ThisComputer,
        ),
        (
            settings_section::WORKSPACE,
            "Name and location",
            SettingsGroup::Workspace,
        ),
        (
            settings_section::PLUGINS,
            "Plugins",
            SettingsGroup::Workspace,
        ),
        (settings_section::SYNC, "Sync", SettingsGroup::Workspace),
        (settings_section::ABOUT, "About Noura", SettingsGroup::App),
        (settings_section::UPDATES, "Updates", SettingsGroup::App),
    ]
    .into_iter()
    .map(|(id, title, group)| SettingsSection {
        id: id.to_owned(),
        title: title.to_owned(),
        group,
    })
    .collect()
}

/// Settings while it is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The ID of the section Settings shows.
    pub section: String,
    /// The Open at login switch, read from the system when Settings
    /// opened.
    pub open_at_login: OpenAtLogin,
    /// What the last action in Settings did: why it failed, or where its
    /// result went. Choosing another section clears it.
    pub notice: Option<Notice>,
    /// The revision of the workspace manifest when Settings read it, so a
    /// rename never overwrites an outside edit made since.
    pub(crate) manifest_revision: Option<String>,
}

/// Noura's version, as this build reports it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Noura's license.
pub const LICENSE: &str = "MIT";

/// A web page the About section links to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AboutLink {
    pub title: &'static str,
    pub url: &'static str,
}

/// The About section's links, in the order it lists them.
pub const ABOUT_LINKS: [AboutLink; 6] = [
    AboutLink {
        title: "Website",
        url: "https://noura.app/",
    },
    AboutLink {
        title: "Source code",
        url: "https://github.com/lobbystack/noura",
    },
    AboutLink {
        title: "Report a problem",
        url: "https://github.com/lobbystack/noura/issues",
    },
    AboutLink {
        title: "Privacy policy",
        url: "https://noura.app/privacy/",
    },
    AboutLink {
        title: "Terms and conditions",
        url: "https://noura.app/terms/",
    },
    THIRD_PARTY_LICENSES,
];

/// The licenses of the code Noura ships. A packaged copy opens the list
/// its package carries (see
/// [`App::set_third_party_licenses`](crate::App::set_third_party_licenses));
/// other builds open this page.
pub const THIRD_PARTY_LICENSES: AboutLink = AboutLink {
    title: "Third-party licenses",
    url: "https://github.com/lobbystack/noura/blob/main/THIRD_PARTY_NOTICES.md",
};

/// Whether `url` is a web link Noura hands to the browser: `http` or
/// `https`, with no spaces or control characters that could split it.
pub(crate) fn is_web_link(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"));
    rest.is_some_and(|rest| {
        !rest.is_empty() && !url.chars().any(|c| c.is_whitespace() || c.is_control())
    })
}
