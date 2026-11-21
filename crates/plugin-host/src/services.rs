//! The services a plugin's capability interfaces call. The app provides
//! them; the host checks each call against the plugin's capabilities first
//! and converts what crosses the WIT boundary.

use std::fmt;

use crate::{
    AiChat, AiCompletion, AiCompletionRequest, AiFailure, AiMessage, AiOrb, AiOrbChanges, AiOrbRun,
    AiProvider, AiRequest, AiRetention, AiRun, AiToolDefinition, AiToolResult, AiWebPage,
    AiWebResult, MenuBarEntry, Notification, PasteMethod, Recording, RecordingOptions,
    ShortcutState, SpeechMicrophone, SpeechModel, SpeechPreferences,
};

/// A workspace-relative path that crossed the plugin boundary: not empty,
/// with `/` between its parts, and no `.`, `..`, `\` or control
/// characters. The service still resolves it inside the workspace and
/// refuses links that leave it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkspacePath(String);

impl WorkspacePath {
    /// Check `path`, or say why it isn't a workspace-relative path.
    pub fn new(path: &str) -> Result<Self, ServiceError> {
        let valid = !path.is_empty()
            && path.len() <= 4096
            && !path.starts_with('/')
            && !path.contains('\\')
            && path.split('/').all(|part| {
                !part.is_empty()
                    && part != "."
                    && part != ".."
                    && !part.chars().any(char::is_control)
            });
        if valid {
            Ok(Self(path.to_owned()))
        } else {
            Err(ServiceError::InvalidPath(path.to_owned()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorkspacePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Why a service refused or failed a call, as the plugin sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceError {
    /// A name a WIT string can't hold, such as one that isn't UTF-8.
    UnsupportedPath,
    /// Not a workspace-relative path, or one inside a folder Noura keeps to
    /// itself.
    InvalidPath(String),
    /// Nothing has this path or ID.
    NotFound(String),
    /// The call failed. The message is for the plugin's author and holds no
    /// raw system details.
    Failed(String),
    /// The file no longer holds the revision the write carried. Holds the
    /// object's ID.
    RevisionConflict(String),
    /// More than one file holds this stable ID.
    IdentityConflict(String),
    /// A property doesn't fit the object type's schema.
    InvalidProperty(PropertyProblem),
    /// Something already has the path the write asked for.
    AlreadyExists(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Folder,
}

/// One file or folder in the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: WorkspacePath,
    pub kind: EntryKind,
    pub object_id: Option<String>,
    pub object_type: Option<String>,
    pub revision: Option<String>,
}

/// An item of a workspace listing: an entry, or a name that can't cross
/// the boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listed {
    Entry(Entry),
    Unsupported,
}

/// A file's bytes and their revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRead {
    pub path: WorkspacePath,
    pub bytes: Vec<u8>,
    pub revision: String,
}

/// An object read by its stable ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectRead {
    pub id: String,
    pub object_type: String,
    pub title: String,
    pub path: WorkspacePath,
    pub body: String,
    /// The frontmatter's other properties, as a JSON object.
    pub properties: String,
    pub revision: String,
}

/// Reads the open workspace for plugins that hold the `workspace-read`
/// capability.
pub trait WorkspaceReads: Send + Sync {
    /// Every file and folder in the workspace.
    fn list_entries(&self) -> Result<Vec<Listed>, ServiceError>;

    /// The file at `path`.
    fn read_file(&self, path: &WorkspacePath) -> Result<FileRead, ServiceError>;

    /// The object with the stable ID `id`.
    fn read_object(&self, id: &str) -> Result<ObjectRead, ServiceError>;

    /// The object the file at `path` holds, even when another file holds
    /// the same ID. A file that holds no object is `NotFound`.
    fn read_object_at(&self, path: &WorkspacePath) -> Result<ObjectRead, ServiceError>;

    /// The link Noura writes for the object with the stable ID `id`, such
    /// as `[[Garden]]`.
    fn link_to(&self, id: &str) -> Result<String, ServiceError> {
        Err(ServiceError::NotFound(id.to_owned()))
    }

    /// The object `link` leads to from the file at `from`, or from the top
    /// of the workspace, or `None` when it leads to no object.
    fn resolve_link(
        &self,
        link: &str,
        from: Option<&WorkspacePath>,
    ) -> Result<Option<ObjectRead>, ServiceError> {
        let _ = (link, from);
        Ok(None)
    }
}

/// A property whose value doesn't fit its object type's schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyProblem {
    pub property: String,
    /// The workspace format's validation code, such as `not_allowed`.
    pub code: String,
}

/// An object of a plugin's own type, as its file holds it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectRecord {
    pub id: String,
    pub object_type: String,
    pub title: String,
    pub path: WorkspacePath,
    pub body: String,
    /// The frontmatter's other properties, as a JSON object.
    pub properties: String,
    pub revision: String,
    /// When the object was created and last changed, as its frontmatter
    /// says.
    pub created: Option<String>,
    pub updated: Option<String>,
    /// The first property, in the schema's order, that doesn't fit the
    /// type's schema. The file stays as it is.
    pub problem: Option<PropertyProblem>,
}

/// A new object of a plugin's own type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewObject {
    pub object_type: String,
    /// Names the file when `path` is `None`. With a path, the path's file
    /// name is the title and this one may be empty.
    pub title: String,
    pub path: Option<WorkspacePath>,
    pub body: String,
    pub properties: serde_json::Map<String, serde_json::Value>,
}

/// What an update changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectChanges {
    pub body: Option<String>,
    /// Properties to set.
    pub properties: serde_json::Map<String, serde_json::Value>,
    /// Properties to remove.
    pub remove: Vec<String>,
}

/// Creates, reads, changes, moves and deletes objects for plugins that
/// hold the `object-types` capability. The host calls it only about types
/// the calling plugin registered. Writes pass the type's schema and return
/// once the file is durably on disk.
pub trait WorkspaceObjects: Send + Sync {
    fn create(&self, object: NewObject) -> Result<ObjectRecord, ServiceError>;

    /// The object with the stable ID `id`, read from its file.
    fn get(&self, id: &str) -> Result<ObjectRecord, ServiceError>;

    /// Every object of `object_type`, read from their files.
    fn list(&self, object_type: &str) -> Result<Vec<ObjectRecord>, ServiceError>;

    fn update(
        &self,
        id: &str,
        expected_revision: &str,
        changes: ObjectChanges,
    ) -> Result<ObjectRecord, ServiceError>;

    fn move_to(
        &self,
        id: &str,
        expected_revision: &str,
        path: &WorkspacePath,
    ) -> Result<ObjectRecord, ServiceError>;

    /// Move the object's file to the trash.
    fn delete(&self, id: &str, expected_revision: &str) -> Result<(), ServiceError>;

    /// Move the folder that holds the object's file to `folder`, with
    /// everything in it. Each Markdown file in it must be an object whose
    /// type `may_write` accepts, and it may hold no folder.
    fn move_folder(
        &self,
        _id: &str,
        _expected_revision: &str,
        _folder: &WorkspacePath,
        _may_write: &dyn Fn(&str) -> bool,
    ) -> Result<ObjectRecord, ServiceError> {
        Err(ServiceError::Failed(
            "Noura can't move folders here.".to_owned(),
        ))
    }
}

/// The instant now and this device's time zone, as a plugin reads them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClockReading {
    /// RFC 3339, in UTC, such as `2026-10-21T13:30:00Z`.
    pub instant: String,
    /// An IANA name such as `America/Toronto`, or, for a zone without one,
    /// its offset from UTC at `instant`, such as `-04:00`.
    pub time_zone: String,
}

impl ClockReading {
    /// The reading at `instant` in `zone`.
    pub fn at(instant: jiff::Timestamp, zone: &jiff::tz::TimeZone) -> Self {
        let time_zone = match zone.iana_name() {
            Some(name) => name.to_owned(),
            None => {
                let seconds = zone.to_offset(instant).seconds();
                let sign = if seconds < 0 { '-' } else { '+' };
                let minutes = seconds.unsigned_abs() / 60;
                format!("{sign}{:02}:{:02}", minutes / 60, minutes % 60)
            }
        };
        Self {
            instant: instant.to_string(),
            time_zone,
        }
    }

    /// This computer's clock and time zone.
    pub fn system() -> Self {
        Self::at(jiff::Timestamp::now(), &jiff::tz::TimeZone::system())
    }
}

/// The clock plugins read: the app's, which tests can fix.
pub trait DeviceClock: Send + Sync {
    fn now(&self) -> ClockReading;
}

/// Opens objects, a plugin's own pages and Settings in the window you
/// acted in, and moves objects to the trash there, for plugins that hold
/// the `views` capability.
pub trait Navigation: Send + Sync {
    /// Open the object with the stable ID `id` in the note editor.
    fn open_object(&self, id: &str, placement: crate::Placement) -> Result<(), ServiceError>;

    /// Show the plugin's own page `page`.
    fn open_page(&self, page: &str) -> Result<(), ServiceError>;

    /// Open Settings on `section`.
    fn open_settings(&self, section: crate::SettingsSection) -> Result<(), ServiceError>;

    /// Move the object with the stable ID `id`, of one of the plugin's own
    /// types, to the trash as the file tree's Move to Trash does. The host
    /// checks the type before it asks.
    fn trash_object(&self, id: &str) -> Result<(), ServiceError>;
}

/// Chats and prompts with the AI providers you set up, for plugins that
/// hold the `ai` capability. The app keeps the providers and their keys
/// on this device and returns no key from any call. Choosing a provider
/// is your say-so, so nothing asks before a request goes out.
pub trait AiChats: Send + Sync {
    /// The providers you set up and turned on.
    fn providers(&self) -> Result<Vec<AiProvider>, ServiceError>;

    /// The workspace's chats, the one changed last first.
    fn chats(&self) -> Result<Vec<AiChat>, ServiceError>;

    /// A chat and its messages, oldest first.
    fn read_chat(&self, id: &str) -> Result<(AiChat, Vec<AiMessage>), ServiceError>;

    /// A new chat, titled `New chat`.
    fn create_chat(&self) -> Result<AiChat, ServiceError>;

    /// Give the chat `id` the title `title`, when its `chat.md` still
    /// holds `expected_revision`.
    fn rename_chat(
        &self,
        id: &str,
        title: &str,
        expected_revision: &str,
    ) -> Result<AiChat, ServiceError>;

    /// Keep the chat `id` as `retention` says, when its `chat.md` still
    /// holds `expected_revision`.
    fn set_retention(
        &self,
        id: &str,
        retention: AiRetention,
        expected_revision: &str,
    ) -> Result<AiChat, ServiceError>;

    /// The tools the plugins on in the workspace register, for the model.
    fn workspace_tools(&self) -> Result<Vec<AiToolDefinition>, ServiceError>;

    /// Write the message and an in-progress answer, then ask the provider.
    fn send(&self, request: AiRequest) -> Result<AiRun, ServiceError>;

    /// Answer the tool calls of the completed run `run` with `results`,
    /// and ask the provider again.
    fn answer_tools(
        &self,
        run: &str,
        results: Vec<AiToolResult>,
        tools: Vec<AiToolDefinition>,
    ) -> Result<AiRun, ServiceError>;

    /// Run the workspace tool calls of the completed run `run`, each once
    /// you allow it, and ask the provider again.
    fn run_tools(&self, run: &str) -> Result<AiRun, ServiceError>;

    /// Answer the last message of yours in the chat `chat` again.
    fn resume(
        &self,
        chat: &str,
        provider: &str,
        tools: Vec<AiToolDefinition>,
    ) -> Result<AiRun, ServiceError>;

    /// The run `id` as it stands now.
    fn read_run(&self, id: &str) -> Result<AiRun, ServiceError>;

    /// Stop the run `id`.
    fn stop(&self, id: &str) -> Result<(), ServiceError>;

    /// Whether the model may search the web and read pages: you added a
    /// web search key. A service without web access says no.
    fn web_ready(&self) -> Result<bool, ServiceError> {
        Ok(false)
    }

    /// Search the web for `query`, for at most `max_results` results. The
    /// failure holds a public code and message, never a key or what the
    /// search provider answered.
    fn web_search(
        &self,
        _query: &str,
        _max_results: Option<u32>,
    ) -> Result<Vec<AiWebResult>, AiFailure> {
        Err(web_unavailable())
    }

    /// Read the page at `url` as text, as [`AiChats::web_search`] fails.
    fn web_fetch(&self, _url: &str) -> Result<AiWebPage, AiFailure> {
        Err(web_unavailable())
    }

    /// Complete `request` with one answer from a provider, writing no
    /// chat. The failure holds a public code and message, never a key or
    /// what the provider answered.
    fn complete(&self, _request: AiCompletionRequest) -> Result<AiCompletion, AiFailure> {
        Err(AiFailure {
            code: "no_provider".to_owned(),
            message: "Add an AI provider in Settings first.".to_owned(),
        })
    }

    /// The default Orb, then the workspace's Orbs by title. A service
    /// without Orbs lists none.
    fn orbs(&self) -> Result<Vec<AiOrb>, ServiceError> {
        Ok(Vec::new())
    }

    /// Create an Orb named `title`.
    fn create_orb(&self, _title: &str) -> Result<AiOrb, ServiceError> {
        Err(no_orbs())
    }

    /// The Orb `id`, its thread and the thread's messages, oldest first.
    fn read_orb(&self, id: &str) -> Result<(AiOrb, Option<AiChat>, Vec<AiMessage>), ServiceError> {
        Err(ServiceError::NotFound(id.to_owned()))
    }

    /// The runs of the Orb `id`, the newest first, at most `limit`.
    fn orb_runs(&self, id: &str, _limit: u32) -> Result<Vec<AiOrbRun>, ServiceError> {
        Err(ServiceError::NotFound(id.to_owned()))
    }

    /// Send `message` to the Orb `id` in its thread. Its tool calls run
    /// without asking.
    fn send_to_orb(&self, id: &str, _message: &str) -> Result<AiRun, ServiceError> {
        Err(ServiceError::NotFound(id.to_owned()))
    }

    /// Run the Orb `id` now, as its schedule would.
    fn run_orb(&self, id: &str) -> Result<AiRun, ServiceError> {
        Err(ServiceError::NotFound(id.to_owned()))
    }

    /// Change the Orb `id`'s file, when it still holds
    /// `expected_revision`.
    fn update_orb(
        &self,
        id: &str,
        _changes: AiOrbChanges,
        _expected_revision: &str,
    ) -> Result<AiOrb, ServiceError> {
        Err(ServiceError::NotFound(id.to_owned()))
    }

    /// Pause the Orb `id`'s triggers, or let them fire again.
    fn set_orb_paused(
        &self,
        id: &str,
        _paused: bool,
        _expected_revision: &str,
    ) -> Result<AiOrb, ServiceError> {
        Err(ServiceError::NotFound(id.to_owned()))
    }

    /// Move the Orb `id`'s folder to the trash.
    fn delete_orb(&self, id: &str, _expected_revision: &str) -> Result<(), ServiceError> {
        Err(ServiceError::NotFound(id.to_owned()))
    }

    /// Whether Noura holds every Orb's triggers after a while away.
    fn orbs_held(&self) -> Result<bool, ServiceError> {
        Ok(false)
    }

    /// Keep the Orbs' triggers running after a while away, or pause every
    /// Orb that has triggers.
    fn keep_orbs_running(&self, _keep: bool) -> Result<(), ServiceError> {
        Err(no_orbs())
    }
}

fn no_orbs() -> ServiceError {
    ServiceError::Failed("Noura can't run Orbs here.".to_owned())
}

/// The events of one plugin that holds `events`. The host checks that the
/// manifest declares each event before the service sees it.
pub trait PluginEvents: Send + Sync {
    /// The event `event` happened, about the object `object` when there is
    /// one. The service starts the Orbs whose triggers name it after the
    /// call returns.
    fn emit(&self, event: &str, object: Option<&str>) -> Result<(), ServiceError>;
}

/// Speech for one plugin that holds the `speech` capability: recording,
/// models and per-device choices. Each call returns at once; recording and
/// transcribing run on the kernel's own threads.
pub trait SpeechService: Send + Sync {
    fn models(&self) -> Result<Vec<SpeechModel>, ServiceError>;
    fn microphones(&self) -> Result<Vec<SpeechMicrophone>, ServiceError>;
    fn preferences(&self) -> Result<SpeechPreferences, ServiceError>;
    fn set_preferences(&self, preferences: SpeechPreferences) -> Result<(), ServiceError>;
    /// Start a recording and return its ID.
    fn start(&self, options: RecordingOptions) -> Result<String, ServiceError>;
    /// The plugin's recording `id`; another plugin's is `NotFound`.
    fn read_recording(&self, id: &str) -> Result<Recording, ServiceError>;
    fn stop(&self, id: &str) -> Result<(), ServiceError>;
    fn cancel(&self, id: &str) -> Result<(), ServiceError>;
    /// Write the finished recording's audio beside the object `object`,
    /// one of the plugin's own types, and return the file's path.
    fn save_audio(&self, id: &str, object: &str) -> Result<WorkspacePath, ServiceError>;
    fn keep_audio(&self, id: &str, days: u32) -> Result<(), ServiceError>;
    fn forget(&self, id: &str) -> Result<(), ServiceError>;
    fn remember_dictation(&self, text: &str) -> Result<(), ServiceError>;
    fn last_dictation(&self) -> Result<Option<String>, ServiceError>;
}

/// The global shortcuts of one plugin that holds `global-shortcuts`, with
/// the keys you set.
pub trait ShortcutService: Send + Sync {
    fn shortcuts(&self) -> Result<Vec<ShortcutState>, ServiceError>;
}

/// Pastes into the focused app for a plugin that holds `paste`. The app
/// refuses a paste outside a call that answers something you did.
pub trait PasteService: Send + Sync {
    fn paste(&self, text: &str, method: PasteMethod) -> Result<(), ServiceError>;
    fn copy(&self, text: &str) -> Result<(), ServiceError>;
}

/// The menu bar or tray entries of one plugin that holds `menu-bar-items`.
pub trait MenuBarService: Send + Sync {
    fn set_entries(&self, entries: Vec<MenuBarEntry>) -> Result<(), ServiceError>;
}

/// The notifications of one plugin that holds `notifications`. The app
/// refuses one outside a call that answers something you did.
pub trait NotificationService: Send + Sync {
    fn notify(&self, notification: Notification) -> Result<(), ServiceError>;
    fn withdraw(&self, id: &str) -> Result<(), ServiceError>;
}

/// What a web call to a service without web access returns.
fn web_unavailable() -> AiFailure {
    AiFailure {
        code: "web_access_unavailable".to_owned(),
        message: "Noura can't search the web here.".to_owned(),
    }
}
