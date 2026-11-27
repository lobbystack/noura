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
    /// Updates, Appearance and Open at login.
    pub const GENERAL: &str = "general";
    /// Text size, line width and spelling.
    pub const EDITOR: &str = "editor";
    /// Every shortcut by area.
    pub const SHORTCUTS: &str = "shortcuts";
    /// The workspace's name and folder.
    pub const WORKSPACE: &str = "workspace";
    /// The plugins installed on this device and enabled in the workspace.
    pub const PLUGINS: &str = "plugins";
    /// Whether the open workspace syncs, and how its sync stands.
    pub const SYNC: &str = "workspace-sync";
    /// The workspace's members and invitations.
    pub const MEMBERS: &str = "workspace-members";
    /// Speech models on this computer, the language you speak, the
    /// plugins' shortcuts and how Noura pastes.
    pub const SPEECH: &str = "speech";
    /// The sync account: the server, this computer's sign-in, your devices
    /// and the recovery kit.
    pub const SYNC_ACCOUNT: &str = "sync-account";
    /// The AI providers on this device, their keys and the Brave Search
    /// key, which the AI plugin's page in Plugins shows. Opening it opens
    /// that page.
    pub const AI: &str = "ai-providers";
    /// Web search, which the AI plugin's page shows. Opening it opens that
    /// page.
    pub const WEB_SEARCH: &str = "web-search";
    /// The patterns of files the workspace leaves out.
    pub const IGNORED_FILES: &str = "ignored-files";
    /// The version, license, links and Copy diagnostics.
    pub const ABOUT: &str = "about";
    /// Check for updates, which General shows. Opening it opens General.
    pub const UPDATES: &str = "updates";
    /// The MCP configuration that connects AI tools outside Noura, which
    /// shows once a client offers the MCP server.
    pub const AI_TOOLS: &str = "ai-tools";
}

/// The plugin whose page in Plugins holds the AI settings when it's
/// installed. Every plugin that uses the AI providers holds them too.
pub const AI_PLUGIN: &str = "ai";

/// The section that shows `id`: General shows Updates, and Plugins shows
/// AI and Web search on the AI plugin's page.
pub(crate) fn section_showing(id: &str) -> &str {
    match id {
        settings_section::UPDATES => settings_section::GENERAL,
        settings_section::AI | settings_section::WEB_SEARCH => settings_section::PLUGINS,
        id => id,
    }
}

/// Whether `id` is the AI settings, which live on a plugin's page in
/// Plugins.
pub(crate) fn shows_ai_settings(id: &str) -> bool {
    matches!(id, settings_section::AI | settings_section::WEB_SEARCH)
}

/// What Search settings finds the AI plugin's page by, though Settings
/// lists no section of its own for it.
pub(crate) fn ai_search_entry() -> &'static SettingsSection {
    static ENTRY: std::sync::LazyLock<SettingsSection> =
        std::sync::LazyLock::new(|| SettingsSection {
            id: settings_section::AI.to_owned(),
            title: "AI".to_owned(),
            group: SettingsGroup::Workspace,
            keywords: section_keywords(settings_section::AI).to_owned(),
        });
    &ENTRY
}

/// One section of Settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsSection {
    pub id: String,
    pub title: String,
    pub group: SettingsGroup,
    /// Other words Search settings finds the section by.
    pub keywords: String,
}

impl SettingsSection {
    /// Whether Search settings shows this section for `query`: its title
    /// or keywords hold the query, ignoring case and outer spaces.
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        format!("{} {}", self.title, self.keywords)
            .to_lowercase()
            .contains(&query)
    }
}

/// Where the kernel's sections and the sections later areas register by
/// a known ID go, in the order Settings lists them. Other sections follow
/// these in their group, in the order they register.
const SECTION_ORDER: [&str; 12] = [
    settings_section::GENERAL,
    settings_section::EDITOR,
    settings_section::SHORTCUTS,
    settings_section::SPEECH,
    settings_section::SYNC_ACCOUNT,
    settings_section::WORKSPACE,
    settings_section::PLUGINS,
    settings_section::AI_TOOLS,
    settings_section::SYNC,
    settings_section::MEMBERS,
    settings_section::IGNORED_FILES,
    settings_section::ABOUT,
];

/// Where the section `id` goes among the known sections, if it is one.
pub(crate) fn section_rank(id: &str) -> Option<usize> {
    SECTION_ORDER.iter().position(|known| *known == id)
}

/// The words Search settings finds a kernel section by, as the legacy
/// app listed them.
pub(crate) fn section_keywords(id: &str) -> &'static str {
    match id {
        settings_section::GENERAL => {
            "appearance theme dark light system startup login updates version sidebar files tree"
        }
        settings_section::EDITOR => "text size font line width spelling spellcheck",
        settings_section::SHORTCUTS => "keys",
        settings_section::SPEECH => {
            "dictation transcription microphone models parakeet whisper language paste"
        }
        settings_section::SYNC_ACCOUNT => "sign in log out disconnect server",
        settings_section::WORKSPACE => "rename folder path",
        settings_section::PLUGINS => "notes tasks calendar projects folders sync",
        settings_section::AI => {
            "ai providers models api keys openai anthropic gemini chatgpt plan web search brave"
        }
        settings_section::AI_TOOLS => "mcp claude cursor assistant model context protocol",
        settings_section::SYNC => "encryption recovery conflicts join devices",
        settings_section::MEMBERS => "invitations roles sharing access members",
        settings_section::IGNORED_FILES => "ignore gitignore exclude hidden files",
        settings_section::ABOUT => "version license privacy terms",
        _ => "",
    }
}

/// The sections every Noura has, in the order Settings lists them.
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
            settings_section::SPEECH,
            "Speech",
            SettingsGroup::ThisComputer,
        ),
        (
            settings_section::SYNC_ACCOUNT,
            "Account",
            SettingsGroup::ThisComputer,
        ),
        (
            settings_section::WORKSPACE,
            "Location",
            SettingsGroup::Workspace,
        ),
        (
            settings_section::PLUGINS,
            "Plugins",
            SettingsGroup::Workspace,
        ),
        (
            settings_section::SYNC,
            "Sync and devices",
            SettingsGroup::Workspace,
        ),
        (
            settings_section::MEMBERS,
            "People",
            SettingsGroup::Workspace,
        ),
        (
            settings_section::IGNORED_FILES,
            "Ignored files",
            SettingsGroup::Workspace,
        ),
        (settings_section::ABOUT, "About Noura", SettingsGroup::App),
    ]
    .into_iter()
    .map(|(id, title, group)| SettingsSection {
        id: id.to_owned(),
        title: title.to_owned(),
        group,
        keywords: section_keywords(id).to_owned(),
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
    /// What Search settings holds. The section list shows only the
    /// sections that match it.
    pub query: String,
    /// The plugin whose details Plugins shows in place of its list.
    pub plugin_details: Option<String>,
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
