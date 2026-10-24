//! Why the host refused to install or load a plugin.

/// What went wrong, as a stable code a client can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InstallErrorCode {
    /// The folder holds no `plugin.toml`.
    ManifestMissing,
    /// `plugin.toml` isn't valid TOML, or a field is missing, unknown or
    /// holds the wrong kind of value.
    InvalidManifest,
    /// The manifest asks for an interface version this host doesn't have.
    UnknownInterfaceVersion,
    /// The manifest declares a capability this host doesn't know.
    UnknownCapability,
    /// The manifest's platforms don't include desktop.
    DesktopNotSupported,
    /// A plugin with this ID is installed on this device already.
    AlreadyInstalled,
    /// The entry component's path leaves the plugin's folder.
    EntryOutsideFolder,
    /// The entry component isn't in the folder or the archive.
    EntryMissing,
    /// The release archive isn't a zip archive Noura can read, or holds a
    /// link, a path that leaves the plugin's folder, or a file too large
    /// to install.
    InvalidArchive,
    /// The entry holds code compiled elsewhere, which the host never runs.
    PrecompiledCode,
    /// The entry isn't a valid WebAssembly component.
    NotAComponent,
    /// The component imports an interface the host never provides, such as
    /// WASI's filesystem.
    ForbiddenImport,
    /// No plugin with this ID is installed on this device.
    NotInstalled,
    /// The plugin's folder or its files have a name Noura can't represent.
    UnsupportedPath,
    /// The host couldn't read or write the plugin's files.
    Storage,
}

impl InstallErrorCode {
    /// The code as clients log and match it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ManifestMissing => "manifest_missing",
            Self::InvalidManifest => "invalid_manifest",
            Self::UnknownInterfaceVersion => "unknown_interface_version",
            Self::UnknownCapability => "unknown_capability",
            Self::DesktopNotSupported => "desktop_not_supported",
            Self::AlreadyInstalled => "already_installed",
            Self::EntryOutsideFolder => "entry_outside_folder",
            Self::EntryMissing => "entry_missing",
            Self::InvalidArchive => "invalid_archive",
            Self::PrecompiledCode => "precompiled_code",
            Self::NotAComponent => "not_a_component",
            Self::ForbiddenImport => "forbidden_import",
            Self::NotInstalled => "not_installed",
            Self::UnsupportedPath => "unsupported_path",
            Self::Storage => "plugin_storage",
        }
    }
}

/// A refused install or load, with a message for the install dialog.
/// Manifest errors carry the line and column they point at.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct InstallError {
    pub code: InstallErrorCode,
    pub message: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

impl InstallError {
    pub(crate) fn new(code: InstallErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            line: None,
            column: None,
        }
    }

    /// A storage failure. The details go to the diagnostic log.
    pub(crate) fn storage(what: &str, error: &dyn std::fmt::Display) -> Self {
        tracing::warn!(%error, what, "a plugin's files couldn't be read or written");
        Self::new(InstallErrorCode::Storage, format!("Noura couldn't {what}."))
    }
}
