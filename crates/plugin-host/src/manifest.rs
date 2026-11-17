//! A plugin's manifest, `plugin.toml` in the plugin's folder: its ID,
//! version, interface version, entry component, capabilities and
//! platforms.

use std::{
    ops::Range,
    path::{Component, Path},
};

use serde::{Deserialize, Serialize};
use toml::Spanned;

use crate::{InstallError, InstallErrorCode};

/// The manifest's file name inside a plugin's folder.
pub const MANIFEST_FILE: &str = "plugin.toml";

/// The `noura:plugin` interface versions this host runs.
pub const INTERFACE_VERSIONS: [&str; 1] = ["0.1.0"];

/// The largest manifest the host reads.
pub(crate) const MAX_MANIFEST_BYTES: u64 = 64 << 10;

/// The longest plugin ID, as the workspace manifest's enabled plugins list
/// allows it.
const MAX_ID_LENGTH: usize = 64;

/// The longest display name.
const MAX_NAME_LENGTH: usize = 80;

/// What a plugin's manifest declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginManifest {
    /// Lowercase letters, digits and hyphens, starting with a letter. The
    /// workspace manifest's enabled plugins list names plugins by it.
    pub id: String,
    /// The name people see.
    pub name: String,
    pub version: semver::Version,
    /// The `noura:plugin` version the plugin was built against.
    pub interface_version: semver::Version,
    /// The entry component's path inside the plugin's folder, with `/`
    /// between its parts.
    pub entry: String,
    pub platforms: Vec<Platform>,
    /// Every capability the plugin may use, in the order the manifest
    /// lists them.
    pub capabilities: Vec<Capability>,
}

/// Where a plugin runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Desktop,
    Mobile,
}

/// A capability a plugin declares and you approve.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "name", rename_all = "kebab-case")]
pub enum Capability {
    /// Read the workspace's files, folders and objects.
    WorkspaceRead,
    /// Add pages to the rail and draw panels in them.
    Views,
    /// Register object types with a properties panel, and create, change,
    /// move and delete objects of those types.
    ObjectTypes,
    /// Connect to these hosts.
    Network { hosts: Vec<String> },
    /// Add commands to the command palette and offer them to other
    /// plugins, and run these commands of other plugins: a plugin's ID for
    /// every command it offers, or `plugin/command` for one.
    Commands {
        #[serde(default)]
        targets: Vec<String>,
    },
    /// Add sections to the Inbox.
    InboxSections,
    /// Add a page to Settings.
    Settings,
    /// Offer tools that AI assistants call through MCP.
    Tools,
    /// Send chats and prompts to the AI providers you set up, and keep the
    /// chats in the workspace.
    Ai,
    /// Record the microphone and system audio and turn speech into text
    /// with models on this device.
    Speech,
    /// Answer these keyboard shortcuts anywhere on the computer, even
    /// while Noura's windows are closed.
    GlobalShortcuts {
        #[serde(default)]
        shortcuts: Vec<ShortcutDeclaration>,
    },
    /// Paste text into the app you're typing in.
    Paste,
    /// Add items to Noura's menu bar or tray menu.
    MenuBarItems,
    /// Show notifications in the system's notification center.
    Notifications,
}

/// A keyboard shortcut a plugin declares under `global-shortcuts`. You can
/// change its keys in Settings; the declaration gives the first ones.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShortcutDeclaration {
    /// Lowercase letters, digits and hyphens, unique within the plugin.
    /// Events name the shortcut by it.
    pub id: String,
    /// What the shortcut does, as Settings lists it, such as `Dictation`.
    pub title: String,
    /// The keys it starts with, such as `Alt+Space`: modifiers among
    /// `Ctrl`, `Alt`, `Shift` and `Cmd`, joined by `+`, then one key.
    pub keys: String,
    /// The keys it starts with on macOS, when they differ from `keys`.
    #[serde(
        default,
        rename = "macos-keys",
        skip_serializing_if = "Option::is_none"
    )]
    pub macos_keys: Option<String>,
    /// The keys it starts with on Windows, when they differ from `keys`.
    #[serde(
        default,
        rename = "windows-keys",
        skip_serializing_if = "Option::is_none"
    )]
    pub windows_keys: Option<String>,
    /// The keys it starts with on Linux, when they differ from `keys`.
    #[serde(
        default,
        rename = "linux-keys",
        skip_serializing_if = "Option::is_none"
    )]
    pub linux_keys: Option<String>,
}

impl ShortcutDeclaration {
    /// The keys it starts with on this computer's system.
    pub fn keys_here(&self) -> &str {
        let here = if cfg!(target_os = "macos") {
            &self.macos_keys
        } else if cfg!(target_os = "windows") {
            &self.windows_keys
        } else {
            &self.linux_keys
        };
        here.as_deref().unwrap_or(&self.keys)
    }
}

/// One thing you approved for a plugin that you can revoke on its own: a
/// capability, or one host or command target of one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Grant {
    /// A whole capability, by the name the manifest uses.
    Capability(String),
    /// One host of the `network` capability.
    Host(String),
    /// One target of the `commands` capability.
    Target(String),
}

impl Capability {
    /// The name the manifest and errors use for it.
    pub fn name(&self) -> &'static str {
        match self {
            Self::WorkspaceRead => "workspace-read",
            Self::Views => "views",
            Self::ObjectTypes => "object-types",
            Self::Network { .. } => "network",
            Self::Commands { .. } => "commands",
            Self::InboxSections => "inbox-sections",
            Self::Settings => "settings",
            Self::Tools => "tools",
            Self::Ai => "ai",
            Self::Speech => "speech",
            Self::GlobalShortcuts { .. } => "global-shortcuts",
            Self::Paste => "paste",
            Self::MenuBarItems => "menu-bar-items",
            Self::Notifications => "notifications",
        }
    }

    /// What approving it lets the plugin do, in plain words.
    pub fn description(&self) -> String {
        self.grants()
            .into_iter()
            .map(|(_, description)| description)
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// What you approve with it, one grant at a time: each host and each
    /// command target on its own, each with what it lets the plugin do.
    pub fn grants(&self) -> Vec<(Grant, String)> {
        let whole = |description: &str| {
            vec![(
                Grant::Capability(self.name().to_owned()),
                description.to_owned(),
            )]
        };
        match self {
            Self::WorkspaceRead => whole("Read the files and folders in your workspace."),
            Self::Views => whole("Add pages to the rail and show panels in them."),
            Self::ObjectTypes => whole(
                "Add its own kinds of objects, show their properties beside them, and create, \
                 change, move and delete them.",
            ),
            Self::Network { hosts } => hosts
                .iter()
                .map(|host| (Grant::Host(host.clone()), format!("Connect to {host}.")))
                .collect(),
            Self::Commands { targets } => {
                let mut grants =
                    whole("Add commands to the command palette and offer them to other plugins.");
                grants.extend(targets.iter().map(|target| {
                    let description = match target.split_once('/') {
                        Some((plugin, command)) => {
                            format!("Run the {command} command of the {plugin} plugin.")
                        }
                        None => format!("Run the commands the {target} plugin offers."),
                    };
                    (Grant::Target(target.clone()), description)
                }));
                grants
            }
            Self::InboxSections => whole("Add sections to the Inbox."),
            Self::Settings => whole("Add a page to Settings."),
            Self::Tools => whole("Offer tools that AI assistants can use."),
            Self::Ai => whole(
                "Send your messages, chats and this workspace's content to the AI providers you \
                 set up, and keep the chats in your workspace. Once you add a Brave Search key, \
                 search the web and read public web pages when the AI asks to.",
            ),
            Self::Speech => whole(
                "Record your microphone and the sound other apps play, and turn speech into \
                 text on this computer.",
            ),
            Self::GlobalShortcuts { shortcuts } => {
                let keys: Vec<String> = shortcuts
                    .iter()
                    .map(|shortcut| format!("{} ({})", shortcut.title, shortcut.keys_here()))
                    .collect();
                whole(&format!(
                    "Answer keyboard shortcuts in any app, even with Noura's windows closed: {}.",
                    keys.join(", ")
                ))
            }
            Self::Paste => whole("Paste text into the app you're typing in."),
            Self::MenuBarItems => whole("Add items to Noura's menu bar menu."),
            Self::Notifications => whole("Show notifications."),
        }
    }

    /// The shortcuts `capabilities` declares.
    pub fn shortcuts(capabilities: &[Self]) -> Vec<ShortcutDeclaration> {
        capabilities
            .iter()
            .flat_map(|capability| match capability {
                Self::GlobalShortcuts { shortcuts } => shortcuts.clone(),
                _ => Vec::new(),
            })
            .collect()
    }

    /// Whether `capabilities` holds the capability called `name`.
    pub fn holds(capabilities: &[Self], name: &str) -> bool {
        capabilities
            .iter()
            .any(|capability| capability.name() == name)
    }

    /// The hosts `capabilities` lets a plugin connect to.
    pub fn hosts(capabilities: &[Self]) -> Vec<String> {
        capabilities
            .iter()
            .flat_map(|capability| match capability {
                Self::Network { hosts } => hosts.clone(),
                _ => Vec::new(),
            })
            .collect()
    }

    /// Whether `capabilities` lets a plugin run the command `command` of
    /// the plugin `plugin`: a target names the command or its plugin.
    pub fn allows_target(capabilities: &[Self], plugin: &str, command: &str) -> bool {
        capabilities.iter().any(|capability| match capability {
            Self::Commands { targets } => {
                targets.iter().any(|target| match target.split_once('/') {
                    Some((p, c)) => p == plugin && c == command,
                    None => target == plugin,
                })
            }
            _ => false,
        })
    }

    /// What `update` declares that `installed` doesn't: whole
    /// capabilities, and the hosts and command targets the installed build
    /// doesn't list. An update that only removes adds nothing.
    pub fn added(installed: &[Self], update: &[Self]) -> Vec<Self> {
        update
            .iter()
            .filter_map(|capability| {
                let Some(known) = installed
                    .iter()
                    .find(|known| known.name() == capability.name())
                else {
                    return Some(capability.clone());
                };
                match (capability, known) {
                    (Self::Network { hosts }, Self::Network { hosts: known }) => {
                        let hosts = missing(hosts, known);
                        (!hosts.is_empty()).then_some(Self::Network { hosts })
                    }
                    (Self::Commands { targets }, Self::Commands { targets: known }) => {
                        let targets = missing(targets, known);
                        (!targets.is_empty()).then_some(Self::Commands { targets })
                    }
                    // A new shortcut, or new keys for one, waits for you.
                    (
                        Self::GlobalShortcuts { shortcuts },
                        Self::GlobalShortcuts { shortcuts: known },
                    ) => shortcuts
                        .iter()
                        .any(|shortcut| !known.contains(shortcut))
                        .then(|| capability.clone()),
                    _ => None,
                }
            })
            .collect()
    }

    /// What a plugin that declares `declared` may use once you approved
    /// `approved`: the capabilities in both, each with the hosts and
    /// targets in both. A network capability left without hosts goes.
    pub fn granted(declared: &[Self], approved: &[Self]) -> Vec<Self> {
        declared
            .iter()
            .filter_map(|capability| {
                let known = approved
                    .iter()
                    .find(|known| known.name() == capability.name())?;
                match (capability, known) {
                    (Self::Network { hosts }, Self::Network { hosts: known }) => {
                        let hosts = shared(hosts, known);
                        (!hosts.is_empty()).then_some(Self::Network { hosts })
                    }
                    (Self::Commands { targets }, Self::Commands { targets: known }) => {
                        Some(Self::Commands {
                            targets: shared(targets, known),
                        })
                    }
                    _ => Some(capability.clone()),
                }
            })
            .collect()
    }

    /// Add `more` to `approved`, merging hosts and targets into the
    /// capabilities approved already.
    pub fn approve(approved: &mut Vec<Self>, more: &[Self]) {
        for capability in more {
            let Some(known) = approved
                .iter_mut()
                .find(|known| known.name() == capability.name())
            else {
                approved.push(capability.clone());
                continue;
            };
            match (capability, known) {
                (Self::Network { hosts: values }, Self::Network { hosts: known })
                | (Self::Commands { targets: values }, Self::Commands { targets: known }) => {
                    for value in values {
                        if !known.contains(value) {
                            known.push(value.clone());
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Take `grant` out of `approved`. Revoking the last host takes the
    /// network capability with it; the commands capability stays without
    /// targets.
    pub fn revoke(approved: &mut Vec<Self>, grant: &Grant) {
        match grant {
            Grant::Capability(name) => approved.retain(|capability| capability.name() != name),
            Grant::Host(host) => {
                for capability in approved.iter_mut() {
                    if let Self::Network { hosts } = capability {
                        hosts.retain(|known| known != host);
                    }
                }
                approved.retain(
                    |capability| !matches!(capability, Self::Network { hosts } if hosts.is_empty()),
                );
            }
            Grant::Target(target) => {
                for capability in approved.iter_mut() {
                    if let Self::Commands { targets } = capability {
                        targets.retain(|known| known != target);
                    }
                }
            }
        }
    }
}

/// The values in `values` that `known` lacks.
fn missing(values: &[String], known: &[String]) -> Vec<String> {
    values
        .iter()
        .filter(|value| !known.contains(value))
        .cloned()
        .collect()
}

/// The values in both lists, in the order of `values`.
fn shared(values: &[String], known: &[String]) -> Vec<String> {
    values
        .iter()
        .filter(|value| known.contains(value))
        .cloned()
        .collect()
}

/// Read and check the manifest in `folder`. The entry component's path
/// must stay inside the folder; [`crate::PluginHost::inspect`] also checks
/// that the file it names is there and isn't a link that leaves it.
pub fn read_manifest(folder: &Path) -> Result<PluginManifest, InstallError> {
    read_manifest_bytes(folder).map(|(manifest, _)| manifest)
}

/// [`read_manifest`], with the bytes it parsed.
pub(crate) fn read_manifest_bytes(
    folder: &Path,
) -> Result<(PluginManifest, Vec<u8>), InstallError> {
    let path = folder.join(MANIFEST_FILE);
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(InstallError::new(
                InstallErrorCode::ManifestMissing,
                format!("The folder has no {MANIFEST_FILE}, so it isn't a plugin."),
            ));
        }
        Err(error) => return Err(InstallError::storage("read plugin.toml", &error)),
    };
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err(InstallError::new(
            InstallErrorCode::InvalidManifest,
            format!("{MANIFEST_FILE} is larger than 64 KB."),
        ));
    }
    let bytes =
        std::fs::read(&path).map_err(|error| InstallError::storage("read plugin.toml", &error))?;
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        InstallError::new(
            InstallErrorCode::InvalidManifest,
            format!("{MANIFEST_FILE} isn't UTF-8 text."),
        )
    })?;
    let manifest = PluginManifest::parse(text)?;
    Ok((manifest, bytes))
}

impl PluginManifest {
    /// Parse and check a manifest's text.
    pub fn parse(text: &str) -> Result<Self, InstallError> {
        let raw: RawManifest = toml::from_str(text).map_err(|error| {
            let at = error.span().map(|span| position(text, span.start));
            located(
                InstallErrorCode::InvalidManifest,
                text,
                error.span(),
                format!(
                    "{MANIFEST_FILE} isn't valid{}: {}",
                    at.map(|(line, column)| format!(" at line {line}, column {column}"))
                        .unwrap_or_default(),
                    error.message().trim_end()
                ),
            )
        })?;
        raw.check(text)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawManifest {
    id: Spanned<String>,
    name: Spanned<String>,
    version: Spanned<String>,
    interface_version: Spanned<String>,
    entry: Spanned<String>,
    platforms: Spanned<Vec<Spanned<String>>>,
    #[serde(default)]
    capabilities: Vec<RawCapability>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCapability {
    name: Spanned<String>,
    hosts: Option<Spanned<Vec<Spanned<String>>>>,
    targets: Option<Spanned<Vec<Spanned<String>>>>,
    shortcuts: Option<Spanned<Vec<Spanned<ShortcutDeclaration>>>>,
}

impl RawManifest {
    fn check(self, text: &str) -> Result<PluginManifest, InstallError> {
        let invalid = |span: Range<usize>, message: String| {
            located(InstallErrorCode::InvalidManifest, text, Some(span), message)
        };
        let id = self.id.get_ref();
        if !valid_plugin_id(id) {
            return Err(invalid(
                self.id.span(),
                format!(
                    "The plugin ID \u{201c}{id}\u{201d} must start with a lowercase letter and \
                     hold only lowercase letters, digits and hyphens, up to {MAX_ID_LENGTH} \
                     characters."
                ),
            ));
        }
        let name = self.name.get_ref().trim();
        if name.is_empty()
            || name.chars().count() > MAX_NAME_LENGTH
            || name.chars().any(char::is_control)
        {
            return Err(invalid(
                self.name.span(),
                format!("The plugin's name must be 1 to {MAX_NAME_LENGTH} characters of text."),
            ));
        }
        let version = semver::Version::parse(self.version.get_ref()).map_err(|_| {
            invalid(
                self.version.span(),
                format!(
                    "The version \u{201c}{}\u{201d} isn't a semantic version such as 1.0.0.",
                    self.version.get_ref()
                ),
            )
        })?;
        let interface_version = semver::Version::parse(self.interface_version.get_ref())
            .ok()
            .filter(|version| {
                INTERFACE_VERSIONS
                    .iter()
                    .any(|known| version.to_string() == *known)
            })
            .ok_or_else(|| {
                located(
                    InstallErrorCode::UnknownInterfaceVersion,
                    text,
                    Some(self.interface_version.span()),
                    format!(
                        "This plugin needs plugin interface {}. This version of Noura runs \
                         interface {}.",
                        self.interface_version.get_ref(),
                        INTERFACE_VERSIONS.join(", ")
                    ),
                )
            })?;
        let entry = self.entry.get_ref();
        if !stays_inside(entry) {
            return Err(located(
                InstallErrorCode::EntryOutsideFolder,
                text,
                Some(self.entry.span()),
                format!(
                    "The entry component \u{201c}{entry}\u{201d} isn't inside the plugin's \
                     folder."
                ),
            ));
        }
        let platforms = platforms(&self.platforms, text)?;
        let mut capabilities = Vec::with_capacity(self.capabilities.len());
        for raw in self.capabilities {
            let span = raw.name.span();
            let capability = raw.check(text)?;
            if capabilities
                .iter()
                .any(|known: &Capability| known.name() == capability.name())
            {
                return Err(invalid(
                    span,
                    format!(
                        "{MANIFEST_FILE} lists the capability {} twice.",
                        capability.name()
                    ),
                ));
            }
            capabilities.push(capability);
        }
        Ok(PluginManifest {
            id: id.clone(),
            name: name.to_owned(),
            version,
            interface_version,
            entry: entry.clone(),
            platforms,
            capabilities,
        })
    }
}

impl RawCapability {
    /// The shortcuts a `global-shortcuts` capability declares: at least
    /// one, each with a valid ID, a title and keys, and no ID twice.
    fn shortcuts(&self, text: &str) -> Result<Vec<ShortcutDeclaration>, InstallError> {
        let invalid = |span: Range<usize>, message: String| {
            located(InstallErrorCode::InvalidManifest, text, Some(span), message)
        };
        let Some(listed) = &self.shortcuts else {
            return Err(invalid(
                self.name.span(),
                "The global-shortcuts capability needs shortcuts.".to_owned(),
            ));
        };
        if listed.get_ref().is_empty() {
            return Err(invalid(
                listed.span(),
                "The global-shortcuts capability needs at least one shortcut.".to_owned(),
            ));
        }
        let mut shortcuts: Vec<ShortcutDeclaration> = Vec::new();
        for shortcut in listed.get_ref() {
            let span = shortcut.span();
            let shortcut = shortcut.get_ref();
            if !valid_plugin_id(&shortcut.id) {
                return Err(invalid(
                    span,
                    format!(
                        "The shortcut ID \u{201c}{}\u{201d} must start with a lowercase letter \
                         and hold only lowercase letters, digits and hyphens.",
                        shortcut.id
                    ),
                ));
            }
            let title = shortcut.title.trim();
            if title.is_empty()
                || title.chars().count() > MAX_NAME_LENGTH
                || title.chars().any(char::is_control)
            {
                return Err(invalid(
                    span,
                    format!(
                        "A shortcut's title must be 1 to {MAX_NAME_LENGTH} characters of text."
                    ),
                ));
            }
            let every_keys = [
                Some(&shortcut.keys),
                shortcut.macos_keys.as_ref(),
                shortcut.windows_keys.as_ref(),
                shortcut.linux_keys.as_ref(),
            ];
            if let Some(keys) = every_keys
                .into_iter()
                .flatten()
                .find(|keys| !valid_keys(keys))
            {
                return Err(invalid(
                    span,
                    format!(
                        "The keys \u{201c}{keys}\u{201d} aren't modifiers among Ctrl, Alt, Shift \
                         and Cmd joined by + and then one key, such as Alt+Space."
                    ),
                ));
            }
            if shortcuts.iter().any(|known| known.id == shortcut.id) {
                return Err(invalid(
                    span,
                    format!("{MANIFEST_FILE} lists the shortcut {} twice.", shortcut.id),
                ));
            }
            shortcuts.push(ShortcutDeclaration {
                id: shortcut.id.clone(),
                title: title.to_owned(),
                keys: shortcut.keys.clone(),
                macos_keys: shortcut.macos_keys.clone(),
                windows_keys: shortcut.windows_keys.clone(),
                linux_keys: shortcut.linux_keys.clone(),
            });
        }
        Ok(shortcuts)
    }

    fn check(&self, text: &str) -> Result<Capability, InstallError> {
        let name = self.name.get_ref().as_str();
        let misplaced = |field: &str, span: Range<usize>| {
            located(
                InstallErrorCode::InvalidManifest,
                text,
                Some(span),
                format!("The {name} capability doesn't take {field}."),
            )
        };
        if name != "network"
            && let Some(hosts) = &self.hosts
        {
            return Err(misplaced("hosts", hosts.span()));
        }
        if name != "commands"
            && let Some(targets) = &self.targets
        {
            return Err(misplaced("targets", targets.span()));
        }
        if name != "global-shortcuts"
            && let Some(shortcuts) = &self.shortcuts
        {
            return Err(misplaced("shortcuts", shortcuts.span()));
        }
        match name {
            "workspace-read" => Ok(Capability::WorkspaceRead),
            "views" => Ok(Capability::Views),
            "object-types" => Ok(Capability::ObjectTypes),
            "network" => Ok(Capability::Network {
                hosts: values(self.hosts.as_ref(), &self.name, text, "hosts", valid_host)?,
            }),
            "commands" => Ok(Capability::Commands {
                targets: match &self.targets {
                    Some(_) => values(
                        self.targets.as_ref(),
                        &self.name,
                        text,
                        "targets",
                        valid_target,
                    )?,
                    None => Vec::new(),
                },
            }),
            "inbox-sections" => Ok(Capability::InboxSections),
            "settings" => Ok(Capability::Settings),
            "tools" => Ok(Capability::Tools),
            "ai" => Ok(Capability::Ai),
            "speech" => Ok(Capability::Speech),
            "paste" => Ok(Capability::Paste),
            "menu-bar-items" => Ok(Capability::MenuBarItems),
            "notifications" => Ok(Capability::Notifications),
            "global-shortcuts" => Ok(Capability::GlobalShortcuts {
                shortcuts: self.shortcuts(text)?,
            }),
            other => Err(located(
                InstallErrorCode::UnknownCapability,
                text,
                Some(self.name.span()),
                format!("{MANIFEST_FILE} asks for a capability Noura doesn't know: {other}."),
            )),
        }
    }
}

/// The hosts or targets a capability lists: at least one, each valid,
/// sorted and without repeats.
fn values(
    list: Option<&Spanned<Vec<Spanned<String>>>>,
    capability: &Spanned<String>,
    text: &str,
    field: &str,
    valid: fn(&str) -> bool,
) -> Result<Vec<String>, InstallError> {
    let name = capability.get_ref();
    let Some(list) = list.filter(|list| !list.get_ref().is_empty()) else {
        return Err(located(
            InstallErrorCode::InvalidManifest,
            text,
            Some(capability.span()),
            format!("The {name} capability needs a list of {field}."),
        ));
    };
    let mut values = Vec::with_capacity(list.get_ref().len());
    for value in list.get_ref() {
        if !valid(value.get_ref()) {
            return Err(located(
                InstallErrorCode::InvalidManifest,
                text,
                Some(value.span()),
                format!(
                    "\u{201c}{}\u{201d} isn't valid in the {name} capability's {field}.",
                    value.get_ref()
                ),
            ));
        }
        values.push(value.get_ref().clone());
    }
    values.sort();
    values.dedup();
    Ok(values)
}

fn platforms(
    list: &Spanned<Vec<Spanned<String>>>,
    text: &str,
) -> Result<Vec<Platform>, InstallError> {
    let mut platforms = Vec::new();
    for platform in list.get_ref() {
        let known = match platform.get_ref().as_str() {
            "desktop" => Platform::Desktop,
            "mobile" => Platform::Mobile,
            other => {
                return Err(located(
                    InstallErrorCode::InvalidManifest,
                    text,
                    Some(platform.span()),
                    format!("Noura doesn't know the platform \u{201c}{other}\u{201d}."),
                ));
            }
        };
        if !platforms.contains(&known) {
            platforms.push(known);
        }
    }
    if !platforms.contains(&Platform::Desktop) {
        return Err(located(
            InstallErrorCode::DesktopNotSupported,
            text,
            Some(list.span()),
            "This plugin doesn't list desktop among its platforms, so it can't run here."
                .to_owned(),
        ));
    }
    Ok(platforms)
}

/// Whether `id` is a plugin ID: a lowercase letter, then lowercase
/// letters, digits or hyphens. The workspace manifest's enabled plugins
/// list accepts the same IDs.
pub fn valid_plugin_id(id: &str) -> bool {
    let mut chars = id.chars();
    id.len() <= MAX_ID_LENGTH
        && chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A host name such as `api.example.com`: lowercase letters, digits,
/// hyphens and dots, with no empty label. A name on the local network,
/// such as `localhost`, `printer.local` or one without a dot, is never a
/// valid host: the web access boundary refuses them all.
fn valid_host(host: &str) -> bool {
    host.len() <= 253
        && host.contains('.')
        && !host.ends_with(".local")
        && !host.ends_with(".localhost")
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}

/// A command target: a plugin's ID, for every command that plugin offers,
/// or `plugin/command` for one.
fn valid_target(target: &str) -> bool {
    match target.split_once('/') {
        Some((plugin, command)) => valid_plugin_id(plugin) && valid_command_id(command),
        None => valid_plugin_id(target),
    }
}

/// Whether `id` can name a plugin's command: the rules of a plugin ID.
pub fn valid_command_id(id: &str) -> bool {
    valid_plugin_id(id)
}

/// Whether a manifest's entry path, or an archive entry's, names a file
/// inside the plugin's folder: relative, with `/` between its parts and no
/// `.` or `..`.
pub(crate) fn stays_inside(entry: &str) -> bool {
    !entry.is_empty()
        && !entry.contains('\\')
        && !entry.starts_with('/')
        && entry.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.contains(':')
                && !part.chars().any(char::is_control)
        })
        && Path::new(entry)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// An error that points at `span` in the manifest.
fn located(
    code: InstallErrorCode,
    text: &str,
    span: Option<Range<usize>>,
    message: String,
) -> InstallError {
    let (line, column) = span.map_or((None, None), |span| {
        let (line, column) = position(text, span.start);
        (Some(line), Some(column))
    });
    InstallError {
        code,
        message,
        line,
        column,
    }
}

/// The line and column, counting from 1, of the byte at `offset`.
fn position(text: &str, offset: usize) -> (usize, usize) {
    let before = text.get(..offset).unwrap_or(text);
    let line = before.matches('\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .map_or(0, |line| line.chars().count())
        + 1;
    (line, column)
}

/// Whether `keys` names a shortcut: zero or more modifiers among `Ctrl`,
/// `Alt`, `Shift` and `Cmd`, each once, joined by `+`, then one key, a
/// letter, a digit, `F1` to `F24` or a named key such as `Space`.
pub fn valid_keys(keys: &str) -> bool {
    let parts: Vec<&str> = keys.split('+').collect();
    let Some((key, modifiers)) = parts.split_last() else {
        return false;
    };
    let mut seen = Vec::new();
    for modifier in modifiers {
        if !MODIFIERS.contains(modifier) || seen.contains(modifier) {
            return false;
        }
        seen.push(*modifier);
    }
    let single = key.len() == 1
        && key
            .chars()
            .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit());
    let function = key
        .strip_prefix('F')
        .and_then(|number| number.parse::<u8>().ok())
        .is_some_and(|number| (1..=24).contains(&number));
    single || function || NAMED_KEYS.contains(key)
}

/// The modifiers a shortcut's keys may hold, in the order Noura writes them.
pub const MODIFIERS: [&str; 4] = ["Ctrl", "Alt", "Shift", "Cmd"];

/// The keys a shortcut may end in besides letters, digits and F1 to F24.
pub const NAMED_KEYS: [&str; 14] = [
    "Space",
    "Enter",
    "Tab",
    "Escape",
    "Backspace",
    "Delete",
    "Up",
    "Down",
    "Left",
    "Right",
    "Home",
    "End",
    "PageUp",
    "PageDown",
];
