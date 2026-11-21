//! One store of a running plugin: the capability interfaces linked into
//! it, the deadline and memory cap of each call, and calls into its
//! exports. [`crate::RunningPlugin`] starts a new store after a call fails.

use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use local_core::{CoreError, HostRequest, HostRequests, WebMethod};
use parking_lot::RwLock;
use wasmtime::component::types::ComponentItem;
use wasmtime::component::{Component, HasSelf, Linker, LinkerInstance, ResourceType};
use wasmtime::{Engine, Store, UpdateDeadline};

use crate::{
    AiChats, Capability, ClockReading, DeviceClock, EntryKind, Limits, Listed, MenuBarService,
    Navigation, NewObject, NotificationService, ObjectChanges, ObjectRead, ObjectRecord,
    ObjectTypes, PasteService, PropertyProblem, Router, SERVICE_WRITTEN_TYPES, ServiceError,
    ShortcutService, SpeechService, TypeError, WorkspaceObjects, WorkspacePath, WorkspaceReads,
    object_types::type_of_id,
    runtime::{EpochTicker, Failure, StoreLimiter, deadline_ticks},
};

mod bindings {
    wasmtime::component::bindgen!({
        path: "../plugin-sdk/wit",
        world: "plugin",
        additional_derives: [PartialEq, Eq],
    });
}

use bindings::Plugin;
pub use bindings::exports::noura::plugin::commands::Command;
pub use bindings::exports::noura::plugin::inbox::Section as InboxSection;
pub use bindings::exports::noura::plugin::kernel_events::{
    KernelEvent, MicrophoneUse, NotificationChoice, ShortcutEvent,
};
pub use bindings::exports::noura::plugin::object_types::ObjectType;
pub use bindings::exports::noura::plugin::tools::Tool;
pub use bindings::exports::noura::plugin::view::Page;
use bindings::exports::noura::plugin::{
    commands as commands_export, inbox as inbox_export, kernel_events as events_export,
    object_types as types_export, settings as settings_export, tools as tools_export,
    view as view_export,
};
pub use bindings::noura::plugin::ai::{
    Chat as AiChat, Completion as AiCompletion, CompletionRequest as AiCompletionRequest,
    Failure as AiFailure, Message as AiMessage, MessageKind as AiMessageKind,
    MessageStatus as AiMessageStatus, Provider as AiProvider, Request as AiRequest,
    Retention as AiRetention, Run as AiRun, RunStatus as AiRunStatus, ToolCall as AiToolCall,
    ToolDefinition as AiToolDefinition, ToolResult as AiToolResult, WebPage as AiWebPage,
    WebResult as AiWebResult,
};
pub use bindings::noura::plugin::global_shortcuts::Shortcut as ShortcutState;
pub use bindings::noura::plugin::menu_bar::{Entry as MenuBarEntry, EntryKind as MenuBarEntryKind};
pub use bindings::noura::plugin::navigation::{Placement, SettingsSection};
pub use bindings::noura::plugin::notifications::{Action as NotificationAction, Notification};
pub use bindings::noura::plugin::paste::Method as PasteMethod;
pub use bindings::noura::plugin::speech::{
    Channel as SpeechChannel, Microphone as SpeechMicrophone, Model as SpeechModel,
    Preferences as SpeechPreferences, Recording, RecordingOptions, RecordingState,
    Segment as SpeechSegment, Source as SpeechSource,
};
pub(crate) use bindings::noura::plugin::types::Error as CallError;
pub use bindings::noura::plugin::ui::{
    Activity, ActivityState, Calendar, CalendarEntry, CalendarMode, Check, Choice, Color, Column,
    ColumnPress, Composer, DatePicker, Editable, EditableStyle, EmptyState, Event, Field, Form,
    Heading, HeadingLevel, Icon, Input, Item, ItemList, MenuItem, Message, MessageAuthor, Move,
    NameField, Naming, Node, NodeKind, Picker, PickerOption, SortOrder, Submission, Table, Target,
    Value, VirtualList, VirtualRow,
};
use bindings::noura::plugin::{
    ai, clock, command_calls, global_shortcuts, menu_bar, navigation, network, notifications,
    objects, paste, speech, types, workspace,
};

/// The capability behind object types, their panels and object writes.
const OBJECT_TYPES: &str = "object-types";

/// The capabilities a running plugin may use now. Revoking one changes
/// this, and the plugin's next call sees the change.
pub(crate) type Grants = Arc<RwLock<Vec<Capability>>>;

/// Why a call into a running plugin failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PluginError {
    /// The plugin doesn't hold the capability the call needs: its
    /// manifest doesn't declare it, or you revoked it. The host made no
    /// call.
    #[error("the plugin doesn't hold the {capability} capability")]
    CapabilityNotDeclared { capability: &'static str },
    #[error("the plugin failed to start: {0}")]
    Instantiate(String),
    #[error("the plugin needs more than its {cap_bytes}-byte memory cap")]
    MemoryCap { cap_bytes: usize },
    /// The call ran past one of its time limits: the plugin's own work
    /// past [`Limits::call_deadline`], or the whole call, waits included,
    /// past [`Limits::max_call_time`]. `deadline` is the limit it passed.
    #[error("the plugin ran past its {deadline:?} deadline")]
    Deadline { deadline: Duration },
    #[error("the plugin failed: {0}")]
    Call(String),
    /// Another call into the plugin was running and didn't finish in
    /// time, or the call came back into a plugin waiting for it.
    #[error("the plugin {plugin} is busy")]
    Busy { plugin: String },
    /// The plugin failed too often and stopped for this session.
    #[error("the plugin failed too often and stopped")]
    Stopped,
    /// The plugin's component doesn't export the interface the call needs.
    #[error("the plugin doesn't export {interface}")]
    NotExported { interface: &'static str },
}

impl PluginError {
    /// Whether the plugin itself failed: it trapped, ran past its deadline
    /// or outgrew its memory cap. The host then starts a new store for its
    /// next call, and counts the failure.
    pub fn is_failure(&self) -> bool {
        matches!(
            self,
            Self::Instantiate(_) | Self::MemoryCap { .. } | Self::Deadline { .. } | Self::Call(_)
        )
    }
}

/// The services a plugin's capability interfaces call. The app provides
/// them; the host checks each call against the plugin's capabilities
/// first.
#[derive(Clone, Default)]
pub struct Services {
    /// Reads the open workspace, for `workspace-read`.
    pub workspace: Option<Arc<dyn WorkspaceReads>>,
    /// Writes objects of the plugin's own types, for `object-types`.
    pub objects: Option<Arc<dyn WorkspaceObjects>>,
    /// The object types the workspace's running plugins hold.
    pub types: ObjectTypes,
    /// Sends requests through `local-core`'s web access boundary, for
    /// `network`.
    pub network: Option<Arc<HostRequests>>,
    /// Finds the plugins whose commands a plugin calls, for the targets of
    /// `commands`.
    pub router: Weak<Router>,
    /// The instant now and this device's time zone. Without one, plugins
    /// read this computer's clock and time zone.
    pub clock: Option<Arc<dyn DeviceClock>>,
    /// Opens objects and the plugin's pages in the window you acted in,
    /// for `views`. Without one, every request fails.
    pub navigation: Option<Arc<dyn Navigation>>,
    /// Chats with the AI providers you set up, for `ai`. Without one,
    /// every call fails.
    pub ai: Option<Arc<dyn AiChats>>,
    /// Recording and speech-to-text, for `speech`. Without one, every
    /// call fails.
    pub speech: Option<Arc<dyn SpeechService>>,
    /// The plugin's global shortcuts, for `global-shortcuts`.
    pub shortcuts: Option<Arc<dyn ShortcutService>>,
    /// Pasting into the focused app, for `paste`.
    pub paste: Option<Arc<dyn PasteService>>,
    /// The plugin's menu bar entries, for `menu-bar-items`.
    pub menu_bar: Option<Arc<dyn MenuBarService>>,
    /// The plugin's notifications, for `notifications`.
    pub notifications: Option<Arc<dyn NotificationService>>,
}

/// How long the current call has run.
#[derive(Clone, Copy)]
struct Clock {
    started: Instant,
    /// Time spent waiting for the network or another plugin, which the
    /// deadline doesn't count.
    waited: Duration,
    /// How long the plugin's own work may take.
    deadline: Duration,
    /// How long the whole call may take, waits included.
    max_time: Duration,
    /// The limit that interrupted the call, once one did.
    passed: Option<Duration>,
}

impl Clock {
    fn start(limits: Limits) -> Self {
        Self {
            started: Instant::now(),
            waited: Duration::ZERO,
            deadline: limits.call_deadline,
            max_time: limits.max_call_time,
            passed: None,
        }
    }

    /// The limit the call has passed, if any: its own work's deadline
    /// first, then the whole call's.
    fn passed(&self) -> Option<Duration> {
        if self.used() >= self.deadline {
            Some(self.deadline)
        } else if self.started.elapsed() >= self.max_time {
            Some(self.max_time)
        } else {
            None
        }
    }

    /// The plugin's own time in the call so far.
    fn used(&self) -> Duration {
        self.started.elapsed().saturating_sub(self.waited)
    }
}

/// What a plugin's store holds: its memory cap, the capabilities it may
/// use, the services they call, and the call in progress.
pub(crate) struct HostState {
    limiter: StoreLimiter,
    grants: Grants,
    services: Services,
    /// The plugins whose calls wait for this one, this plugin last.
    chain: Vec<String>,
    clock: Clock,
    /// The plugin's ID, which owns the types it registers.
    plugin: String,
    /// The running plugin's token, which holds its types until it stops.
    instance: u64,
}

impl HostState {
    fn holds(&self, name: &str) -> bool {
        Capability::holds(&self.grants.read(), name)
    }

    /// The workspace service, when the plugin holds `workspace-read`.
    fn workspace(&self) -> Result<&dyn WorkspaceReads, types::Error> {
        if !self.holds("workspace-read") {
            return Err(types::Error::CapabilityNotDeclared(
                "workspace-read".to_owned(),
            ));
        }
        self.services
            .workspace
            .as_deref()
            .ok_or_else(|| types::Error::Failed("No workspace is open.".to_owned()))
    }

    /// Run `wait`, which waits for something outside the plugin, without
    /// counting the time toward the call's deadline.
    fn waiting<R>(&mut self, wait: impl FnOnce(&mut Self) -> R) -> R {
        let started = Instant::now();
        let result = wait(self);
        self.clock.waited += started.elapsed();
        result
    }

    /// The object service, when the plugin holds `object-types` and
    /// registered `object_type`. A type the plugin doesn't hold, a note
    /// for one, gets the capability error: the capability covers the
    /// plugin's own types only.
    fn objects(&self, object_type: &str) -> Result<&dyn WorkspaceObjects, types::Error> {
        let refused = || types::Error::CapabilityNotDeclared(OBJECT_TYPES.to_owned());
        if !self.holds(OBJECT_TYPES) || !self.services.types.held_by(object_type, self.instance) {
            return Err(refused());
        }
        self.services
            .objects
            .as_deref()
            .ok_or_else(|| types::Error::Failed("No workspace is open.".to_owned()))
    }

    /// The navigation service, when the plugin holds `views`.
    fn navigation(&self) -> Result<&dyn Navigation, types::Error> {
        if !self.holds("views") {
            return Err(types::Error::CapabilityNotDeclared("views".to_owned()));
        }
        self.services.navigation.as_deref().ok_or_else(|| {
            types::Error::Failed("Noura can't open anything for this plugin here.".to_owned())
        })
    }

    /// The AI service, when the plugin holds `ai`.
    fn ai(&self) -> Result<Arc<dyn AiChats>, types::Error> {
        if !self.holds("ai") {
            return Err(types::Error::CapabilityNotDeclared("ai".to_owned()));
        }
        self.services
            .ai
            .clone()
            .ok_or_else(|| types::Error::Failed("Noura can't reach AI providers here.".to_owned()))
    }

    /// Run `call` on the AI service. Reading and writing chat files is
    /// waiting, which the call's deadline doesn't count.
    fn with_ai<R>(
        &mut self,
        call: impl FnOnce(&dyn AiChats) -> Result<R, ServiceError>,
    ) -> Result<R, types::Error> {
        let ai = self.ai()?;
        self.waiting(|_| call(ai.as_ref())).map_err(wit_error)
    }

    /// The service `pick` takes from the services, when the plugin holds
    /// `capability`.
    fn service<T: ?Sized>(
        &self,
        capability: &str,
        pick: impl FnOnce(&Services) -> Option<Arc<T>>,
    ) -> Result<Arc<T>, types::Error> {
        if !self.holds(capability) {
            return Err(types::Error::CapabilityNotDeclared(capability.to_owned()));
        }
        pick(&self.services)
            .ok_or_else(|| types::Error::Failed(format!("Noura can't offer {capability} here.")))
    }

    /// Run `call` on the speech service. Starting a recording opens a
    /// device, which the call's deadline doesn't count.
    fn with_speech<R>(
        &mut self,
        call: impl FnOnce(&dyn SpeechService) -> Result<R, ServiceError>,
    ) -> Result<R, types::Error> {
        let speech = self.service("speech", |services| services.speech.clone())?;
        self.waiting(|_| call(speech.as_ref())).map_err(wit_error)
    }

    /// The object service for the object with the stable ID `id`, whose
    /// type is its prefix.
    fn objects_for(&self, id: &str) -> Result<&dyn WorkspaceObjects, types::Error> {
        let object_type = type_of_id(id).unwrap_or_default();
        self.objects(object_type)
    }

    /// The object service for a write to an object of `object_type`.
    /// Files of a type a host service writes, such as a chat, change only
    /// through that service, so the write is refused before it reaches
    /// any file.
    fn objects_to_write(&self, object_type: &str) -> Result<&dyn WorkspaceObjects, types::Error> {
        let service = self.objects(object_type)?;
        if SERVICE_WRITTEN_TYPES.contains(&object_type) {
            return Err(types::Error::Failed(
                "Noura writes chats and their messages only through the ai interface.".to_owned(),
            ));
        }
        Ok(service)
    }

    /// [`Self::objects_to_write`] for the object with the stable ID `id`.
    fn objects_to_write_for(&self, id: &str) -> Result<&dyn WorkspaceObjects, types::Error> {
        let object_type = type_of_id(id).unwrap_or_default();
        self.objects_to_write(object_type)
    }
}

impl types::Host for HostState {}

impl bindings::noura::plugin::ui::Host for HostState {}

impl workspace::Host for HostState {
    fn list_entries(
        &mut self,
    ) -> Result<Vec<Result<workspace::Entry, types::Error>>, types::Error> {
        let listed = self.workspace()?.list_entries().map_err(wit_error)?;
        Ok(listed
            .into_iter()
            .map(|listed| match listed {
                Listed::Entry(entry) => Ok(workspace::Entry {
                    path: entry.path.as_str().to_owned(),
                    kind: match entry.kind {
                        EntryKind::File => workspace::EntryKind::File,
                        EntryKind::Folder => workspace::EntryKind::Folder,
                    },
                    object_id: entry.object_id,
                    object_type: entry.object_type,
                    revision: entry.revision,
                }),
                Listed::Unsupported => Err(types::Error::UnsupportedPath),
            })
            .collect())
    }

    fn read_file(&mut self, path: String) -> Result<workspace::File, types::Error> {
        let workspace = self.workspace()?;
        let path = WorkspacePath::new(&path).map_err(wit_error)?;
        let file = workspace.read_file(&path).map_err(wit_error)?;
        Ok(workspace::File {
            path: file.path.as_str().to_owned(),
            bytes: file.bytes,
            revision: file.revision,
        })
    }

    fn read_object(&mut self, id: String) -> Result<workspace::Object, types::Error> {
        let object = self.workspace()?.read_object(&id).map_err(wit_error)?;
        Ok(object_of(object))
    }

    fn read_object_at(&mut self, path: String) -> Result<workspace::Object, types::Error> {
        let workspace = self.workspace()?;
        let path = WorkspacePath::new(&path).map_err(wit_error)?;
        let object = workspace.read_object_at(&path).map_err(wit_error)?;
        Ok(object_of(object))
    }

    fn link_to(&mut self, id: String) -> Result<String, types::Error> {
        self.workspace()?.link_to(&id).map_err(wit_error)
    }

    fn resolve_link(
        &mut self,
        link: String,
        from: Option<String>,
    ) -> Result<Option<workspace::Object>, types::Error> {
        let workspace = self.workspace()?;
        let from = from
            .map(|path| WorkspacePath::new(&path))
            .transpose()
            .map_err(wit_error)?;
        let object = workspace
            .resolve_link(&link, from.as_ref())
            .map_err(wit_error)?;
        Ok(object.map(object_of))
    }
}

impl speech::Host for HostState {
    fn models(&mut self) -> Result<Vec<speech::Model>, types::Error> {
        self.with_speech(|speech| speech.models())
    }

    fn microphones(&mut self) -> Result<Vec<speech::Microphone>, types::Error> {
        self.with_speech(|speech| speech.microphones())
    }

    fn read_preferences(&mut self) -> Result<speech::Preferences, types::Error> {
        self.with_speech(|speech| speech.preferences())
    }

    fn set_preferences(&mut self, preferences: speech::Preferences) -> Result<(), types::Error> {
        self.with_speech(|speech| speech.set_preferences(preferences))
    }

    fn start(&mut self, options: speech::RecordingOptions) -> Result<String, types::Error> {
        if options.sources.is_empty() {
            return Err(types::Error::Failed(
                "A recording needs at least one source.".to_owned(),
            ));
        }
        self.with_speech(|speech| speech.start(options))
    }

    fn read_recording(&mut self, id: String) -> Result<speech::Recording, types::Error> {
        self.with_speech(|speech| speech.read_recording(&id))
    }

    fn stop(&mut self, id: String) -> Result<(), types::Error> {
        self.with_speech(|speech| speech.stop(&id))
    }

    fn cancel(&mut self, id: String) -> Result<(), types::Error> {
        self.with_speech(|speech| speech.cancel(&id))
    }

    fn save_audio(&mut self, id: String, object: String) -> Result<String, types::Error> {
        // The audio goes beside one of the plugin's own objects only.
        let object_type = type_of_id(&object).unwrap_or_default().to_owned();
        if !self.holds(OBJECT_TYPES) || !self.services.types.held_by(&object_type, self.instance) {
            return Err(types::Error::CapabilityNotDeclared(OBJECT_TYPES.to_owned()));
        }
        self.with_speech(|speech| speech.save_audio(&id, &object))
            .map(|path| path.as_str().to_owned())
    }

    fn keep_audio(&mut self, id: String, days: u32) -> Result<(), types::Error> {
        self.with_speech(|speech| speech.keep_audio(&id, days))
    }

    fn forget(&mut self, id: String) -> Result<(), types::Error> {
        self.with_speech(|speech| speech.forget(&id))
    }

    fn remember_dictation(&mut self, text: String) -> Result<(), types::Error> {
        self.with_speech(|speech| speech.remember_dictation(&text))
    }

    fn last_dictation(&mut self) -> Result<Option<String>, types::Error> {
        self.with_speech(|speech| speech.last_dictation())
    }
}

impl global_shortcuts::Host for HostState {
    fn shortcuts(&mut self) -> Result<Vec<global_shortcuts::Shortcut>, types::Error> {
        let service = self.service("global-shortcuts", |services| services.shortcuts.clone())?;
        service.shortcuts().map_err(wit_error)
    }
}

impl paste::Host for HostState {
    fn paste(&mut self, text: String, method: paste::Method) -> Result<(), types::Error> {
        let service = self.service("paste", |services| services.paste.clone())?;
        // Waiting for the target app to read the clipboard doesn't count
        // toward the deadline.
        self.waiting(|_| service.paste(&text, method))
            .map_err(wit_error)
    }

    fn copy(&mut self, text: String) -> Result<(), types::Error> {
        let service = self.service("paste", |services| services.paste.clone())?;
        service.copy(&text).map_err(wit_error)
    }
}

impl menu_bar::Host for HostState {
    fn set_entries(&mut self, entries: Vec<menu_bar::Entry>) -> Result<(), types::Error> {
        let service = self.service("menu-bar-items", |services| services.menu_bar.clone())?;
        check_menu_entries(&entries)?;
        service.set_entries(entries).map_err(wit_error)
    }
}

impl notifications::Host for HostState {
    fn notify(&mut self, notification: Notification) -> Result<(), types::Error> {
        let service = self.service("notifications", |services| services.notifications.clone())?;
        check_notification(&notification)?;
        service.notify(notification).map_err(wit_error)
    }

    fn withdraw(&mut self, id: String) -> Result<(), types::Error> {
        let service = self.service("notifications", |services| services.notifications.clone())?;
        service.withdraw(&id).map_err(wit_error)
    }
}

/// The most actions one notification holds.
const MAX_NOTIFICATION_ACTIONS: usize = 3;

/// The longest title, body, ID or action label of a notification, in
/// characters.
const MAX_NOTIFICATION_TEXT: usize = 300;

/// A notification shows a title of text, at most three actions with
/// distinct IDs and labels of text, and nothing too long.
fn check_notification(notification: &Notification) -> Result<(), types::Error> {
    let refused = |message: &str| Err(types::Error::Failed(message.to_owned()));
    let text = |value: &str| {
        value.chars().count() <= MAX_NOTIFICATION_TEXT && !value.chars().any(char::is_control)
    };
    if notification.id.is_empty() || !text(&notification.id) {
        return refused("A notification needs an ID of text.");
    }
    if notification.title.trim().is_empty() || !text(&notification.title) {
        return refused("A notification needs a title of text.");
    }
    if !text(&notification.body) {
        return refused("A notification's body is one line of text.");
    }
    if notification.actions.len() > MAX_NOTIFICATION_ACTIONS {
        return refused("A notification holds at most 3 actions.");
    }
    for (index, action) in notification.actions.iter().enumerate() {
        if action.id.is_empty()
            || !text(&action.id)
            || action.label.trim().is_empty()
            || !text(&action.label)
        {
            return refused("A notification's action needs an ID and a label of text.");
        }
        if notification.actions[..index]
            .iter()
            .any(|known| known.id == action.id)
        {
            return refused("A notification lists an action twice.");
        }
    }
    Ok(())
}

/// The most entries a plugin may show in the menu bar menu.
const MAX_MENU_ENTRIES: usize = 200;

/// Refuse entries that can't make a menu: an ID twice, an empty label on
/// an item or a submenu, or a parent that isn't an earlier submenu.
fn check_menu_entries(entries: &[menu_bar::Entry]) -> Result<(), types::Error> {
    let refuse = |message: String| Err(types::Error::Failed(message));
    if entries.len() > MAX_MENU_ENTRIES {
        return refuse(format!(
            "A plugin shows at most {MAX_MENU_ENTRIES} menu entries."
        ));
    }
    let mut seen: Vec<(&str, bool)> = Vec::new();
    for entry in entries {
        if entry.id.is_empty() || seen.iter().any(|(id, _)| *id == entry.id) {
            return refuse(format!(
                "The menu entry ID \u{201c}{}\u{201d} is empty or taken.",
                entry.id
            ));
        }
        let separator = matches!(entry.kind, menu_bar::EntryKind::Separator);
        if !separator && entry.label.trim().is_empty() {
            return refuse(format!("The menu entry {} needs a label.", entry.id));
        }
        if let Some(parent) = &entry.parent
            && !seen.iter().any(|(id, submenu)| id == parent && *submenu)
        {
            return refuse(format!(
                "The menu entry {} names {parent}, which isn't an earlier submenu.",
                entry.id
            ));
        }
        seen.push((
            &entry.id,
            matches!(entry.kind, menu_bar::EntryKind::Submenu),
        ));
    }
    Ok(())
}

/// An object read from the workspace, as it crosses the boundary.
fn object_of(object: ObjectRead) -> workspace::Object {
    workspace::Object {
        id: object.id,
        object_type: object.object_type,
        title: object.title,
        path: object.path.as_str().to_owned(),
        body: object.body,
        properties: object.properties,
        revision: object.revision,
    }
}

impl network::Host for HostState {
    fn fetch(&mut self, request: network::Request) -> Result<network::Response, types::Error> {
        let hosts = {
            let grants = self.grants.read();
            if !Capability::holds(&grants, "network") {
                return Err(types::Error::CapabilityNotDeclared("network".to_owned()));
            }
            Capability::hosts(&grants)
        };
        let Some(requests) = self.services.network.clone() else {
            return Err(types::Error::Failed(
                "Noura can't send network requests here.".to_owned(),
            ));
        };
        let request = HostRequest {
            method: match request.method {
                network::Method::Get => WebMethod::Get,
                network::Method::Head => WebMethod::Head,
                network::Method::Post => WebMethod::Post,
                network::Method::Put => WebMethod::Put,
                network::Method::Patch => WebMethod::Patch,
                network::Method::Delete => WebMethod::Delete,
            },
            url: request.url,
            headers: request
                .headers
                .into_iter()
                .map(|header| (header.name, header.value))
                .collect(),
            body: request.body,
        };
        let response = self.waiting(|_| {
            requests
                .send_blocking(request, &hosts)
                .map_err(network_error)
        })?;
        Ok(network::Response {
            status: response.status,
            url: response.url,
            headers: response
                .headers
                .into_iter()
                .map(|(name, value)| network::Header { name, value })
                .collect(),
            body: response.body,
        })
    }
}

impl command_calls::Host for HostState {
    fn call(
        &mut self,
        plugin: String,
        command: String,
        input: String,
    ) -> Result<String, types::Error> {
        {
            let grants = self.grants.read();
            if !Capability::holds(&grants, "commands") {
                return Err(types::Error::CapabilityNotDeclared("commands".to_owned()));
            }
            if !Capability::allows_target(&grants, &plugin, &command) {
                return Err(types::Error::TargetNotDeclared(format!(
                    "{plugin}/{command}"
                )));
            }
        }
        let Some(router) = self.services.router.upgrade() else {
            return Err(types::Error::TargetUnavailable(format!(
                "{plugin}/{command}"
            )));
        };
        let chain = self.chain.clone();
        self.waiting(|_| router.call(&chain, &plugin, &command, &input))
    }
}

impl clock::Host for HostState {
    fn now(&mut self) -> clock::Reading {
        let reading = match &self.services.clock {
            Some(clock) => clock.now(),
            None => ClockReading::system(),
        };
        clock::Reading {
            instant: reading.instant,
            time_zone: reading.time_zone,
        }
    }
}

impl navigation::Host for HostState {
    fn open_object(
        &mut self,
        id: String,
        placement: navigation::Placement,
    ) -> Result<(), types::Error> {
        self.navigation()?
            .open_object(&id, placement)
            .map_err(wit_error)
    }

    fn open_page(&mut self, page: String) -> Result<(), types::Error> {
        self.navigation()?.open_page(&page).map_err(wit_error)
    }

    fn open_settings(&mut self, section: navigation::SettingsSection) -> Result<(), types::Error> {
        // A plugin with a page in Settings may open Settings too.
        let navigation = if self.holds("settings") && !self.holds("views") {
            self.services.navigation.as_deref().ok_or_else(|| {
                types::Error::Failed("Noura can't open anything for this plugin here.".to_owned())
            })?
        } else {
            self.navigation()?
        };
        navigation.open_settings(section).map_err(wit_error)
    }

    fn trash_object(&mut self, id: String) -> Result<(), types::Error> {
        // Only objects of the plugin's own types, as for its writes.
        self.objects_for(&id)?;
        self.navigation()?.trash_object(&id).map_err(wit_error)
    }
}

impl ai::Host for HostState {
    fn providers(&mut self) -> Result<Vec<ai::Provider>, types::Error> {
        self.with_ai(|ai| ai.providers())
    }

    fn chats(&mut self) -> Result<Vec<ai::Chat>, types::Error> {
        self.with_ai(|ai| ai.chats())
    }

    fn read_chat(&mut self, id: String) -> Result<(ai::Chat, Vec<ai::Message>), types::Error> {
        self.with_ai(|ai| ai.read_chat(&id))
    }

    fn create_chat(&mut self) -> Result<ai::Chat, types::Error> {
        self.with_ai(|ai| ai.create_chat())
    }

    fn rename_chat(
        &mut self,
        id: String,
        title: String,
        expected_revision: String,
    ) -> Result<ai::Chat, types::Error> {
        self.with_ai(|ai| ai.rename_chat(&id, &title, &expected_revision))
    }

    fn set_retention(
        &mut self,
        id: String,
        retention: ai::Retention,
        expected_revision: String,
    ) -> Result<ai::Chat, types::Error> {
        self.with_ai(|ai| ai.set_retention(&id, retention, &expected_revision))
    }

    fn workspace_tools(&mut self) -> Result<Vec<ai::ToolDefinition>, types::Error> {
        self.with_ai(|ai| ai.workspace_tools())
    }

    fn send(&mut self, request: ai::Request) -> Result<ai::Run, types::Error> {
        self.with_ai(|ai| ai.send(request))
    }

    fn answer_tools(
        &mut self,
        run: String,
        results: Vec<ai::ToolResult>,
        tools: Vec<ai::ToolDefinition>,
    ) -> Result<ai::Run, types::Error> {
        self.with_ai(|ai| ai.answer_tools(&run, results, tools))
    }

    fn run_tools(&mut self, run: String) -> Result<ai::Run, types::Error> {
        self.with_ai(|ai| ai.run_tools(&run))
    }

    fn resume(
        &mut self,
        chat: String,
        provider: String,
        tools: Vec<ai::ToolDefinition>,
    ) -> Result<ai::Run, types::Error> {
        self.with_ai(|ai| ai.resume(&chat, &provider, tools))
    }

    fn read_run(&mut self, id: String) -> Result<ai::Run, types::Error> {
        self.with_ai(|ai| ai.read_run(&id))
    }

    fn stop(&mut self, id: String) -> Result<(), types::Error> {
        self.with_ai(|ai| ai.stop(&id))
    }

    fn web_ready(&mut self) -> Result<bool, types::Error> {
        self.with_ai(|ai| ai.web_ready())
    }

    fn web_search(
        &mut self,
        query: String,
        max_results: Option<u32>,
    ) -> Result<Vec<ai::WebResult>, ai::WebError> {
        // The request goes out from the app, through local-core's web
        // access boundary: the plugin has no socket of its own.
        let ai = self.ai().map_err(ai::WebError::Refused)?;
        self.waiting(|_| ai.web_search(&query, max_results))
            .map_err(ai::WebError::Failed)
    }

    fn web_fetch(&mut self, url: String) -> Result<ai::WebPage, ai::WebError> {
        let ai = self.ai().map_err(ai::WebError::Refused)?;
        self.waiting(|_| ai.web_fetch(&url))
            .map_err(ai::WebError::Failed)
    }

    fn complete(
        &mut self,
        request: ai::CompletionRequest,
    ) -> Result<ai::Completion, ai::CompletionError> {
        let ai = self.ai().map_err(ai::CompletionError::Refused)?;
        if request.prompt.trim().is_empty() {
            return Err(ai::CompletionError::Failed(ai::Failure {
                code: "empty_prompt".to_owned(),
                message: "The prompt is empty.".to_owned(),
            }));
        }
        self.waiting(|_| ai.complete(request))
            .map_err(ai::CompletionError::Failed)
    }
}

impl objects::Host for HostState {
    fn create(&mut self, object: objects::NewObject) -> Result<objects::Object, types::Error> {
        let service = self.objects_to_write(&object.object_type)?;
        let path = object
            .path
            .as_deref()
            .map(WorkspacePath::new)
            .transpose()
            .map_err(wit_error)?;
        let new = NewObject {
            object_type: object.object_type,
            title: object.title,
            path,
            body: object.body,
            properties: json_object(&object.properties)?,
        };
        service.create(new).map(wit_object).map_err(wit_error)
    }

    fn get(&mut self, id: String) -> Result<objects::Object, types::Error> {
        self.objects_for(&id)?
            .get(&id)
            .map(wit_object)
            .map_err(wit_error)
    }

    fn list(&mut self, object_type: String) -> Result<Vec<objects::Object>, types::Error> {
        let listed = self
            .objects(&object_type)?
            .list(&object_type)
            .map_err(wit_error)?;
        Ok(listed.into_iter().map(wit_object).collect())
    }

    fn update(
        &mut self,
        id: String,
        expected_revision: String,
        changes: objects::Changes,
    ) -> Result<objects::Object, types::Error> {
        let service = self.objects_to_write_for(&id)?;
        let changes = ObjectChanges {
            body: changes.body,
            properties: json_object(&changes.properties)?,
            remove: changes.remove,
        };
        service
            .update(&id, &expected_revision, changes)
            .map(wit_object)
            .map_err(wit_error)
    }

    fn move_to(
        &mut self,
        id: String,
        expected_revision: String,
        path: String,
    ) -> Result<objects::Object, types::Error> {
        let service = self.objects_to_write_for(&id)?;
        let path = WorkspacePath::new(&path).map_err(wit_error)?;
        service
            .move_to(&id, &expected_revision, &path)
            .map(wit_object)
            .map_err(wit_error)
    }

    fn delete(&mut self, id: String, expected_revision: String) -> Result<(), types::Error> {
        self.objects_to_write_for(&id)?
            .delete(&id, &expected_revision)
            .map_err(wit_error)
    }

    fn move_folder(
        &mut self,
        id: String,
        expected_revision: String,
        folder: String,
    ) -> Result<objects::Object, types::Error> {
        let service = self.objects_to_write_for(&id)?;
        let folder = WorkspacePath::new(&folder).map_err(wit_error)?;
        // The folder's objects must be ones the plugin may write itself.
        let may_write = |object_type: &str| {
            self.services.types.held_by(object_type, self.instance)
                && !SERVICE_WRITTEN_TYPES.contains(&object_type)
        };
        service
            .move_folder(&id, &expected_revision, &folder, &may_write)
            .map(wit_object)
            .map_err(wit_error)
    }
}

/// The JSON object `text` holds, or why it isn't one.
fn json_object(text: &str) -> Result<serde_json::Map<String, serde_json::Value>, types::Error> {
    if text.trim().is_empty() {
        return Ok(serde_json::Map::new());
    }
    match serde_json::from_str(text) {
        Ok(serde_json::Value::Object(map)) => Ok(map),
        _ => Err(types::Error::Failed(
            "Properties must be a JSON object.".to_owned(),
        )),
    }
}

fn wit_object(object: ObjectRecord) -> objects::Object {
    objects::Object {
        id: object.id,
        object_type: object.object_type,
        title: object.title,
        path: object.path.as_str().to_owned(),
        body: object.body,
        properties: object.properties,
        revision: object.revision,
        created: object.created,
        updated: object.updated,
        problem: object.problem.map(wit_problem),
    }
}

fn wit_problem(problem: PropertyProblem) -> types::PropertyError {
    types::PropertyError {
        property: problem.property,
        code: problem.code,
    }
}

fn wit_error(error: ServiceError) -> types::Error {
    match error {
        ServiceError::UnsupportedPath => types::Error::UnsupportedPath,
        ServiceError::InvalidPath(path) => types::Error::InvalidPath(path),
        ServiceError::NotFound(what) => types::Error::NotFound(what),
        ServiceError::Failed(message) => types::Error::Failed(message),
        ServiceError::RevisionConflict(id) => types::Error::RevisionConflict(id),
        ServiceError::IdentityConflict(id) => types::Error::IdentityConflict(id),
        ServiceError::InvalidProperty(problem) => {
            types::Error::InvalidProperty(wit_problem(problem))
        }
        ServiceError::AlreadyExists(path) => types::Error::AlreadyExists(path),
    }
}

/// A refused or failed network request as the plugin sees it. The web
/// access boundary's messages hold no raw system details.
fn network_error(error: CoreError) -> types::Error {
    match error.code.as_str() {
        "web_url_not_https"
        | "web_url_invalid"
        | "web_url_unsafe_host"
        | "web_host_not_allowed"
        | "web_redirect_limit_exceeded"
        | "web_redirect_invalid"
        | "web_header_invalid" => types::Error::NetworkRefused(error.message),
        _ => types::Error::Failed(error.message),
    }
}

/// The interfaces a plugin's component exports. Each is optional: a plugin
/// that doesn't export one registers nothing under it.
struct Exports {
    events: Option<events_export::Guest>,
    view: Option<view_export::Guest>,
    object_types: Option<types_export::Guest>,
    commands: Option<commands_export::Guest>,
    inbox: Option<inbox_export::Guest>,
    settings: Option<settings_export::Guest>,
    tools: Option<tools_export::Guest>,
}

/// Loads the exported interface `$name`, whose indices `$module` looks
/// up: `None` when the component doesn't export it, and an error that
/// stops the start when it exports it without one of its functions or
/// with the wrong types.
macro_rules! export {
    ($module:ident, $name:literal, $pre:expr, $store:expr, $instance:expr) => {
        if $pre.component().get_export_index(None, $name).is_some() {
            Some($module::GuestIndices::new($pre)?.load(&mut *$store, $instance)?)
        } else {
            None
        }
    };
}

/// Makes imports the host doesn't provide, such as the WASI interfaces the
/// standard library pulls in for printing, trap when called. Noura's own
/// interfaces stay out: a plugin built against an earlier version links to
/// the version the host serves by the linker's semver rule, and a stub
/// under the plugin's version would stand in front of it.
pub fn trap_foreign_imports<T: 'static>(
    linker: &mut Linker<T>,
    component: &Component,
) -> wasmtime::Result<()> {
    let engine = component.engine();
    let plugin = component.component_type();
    for (name, import) in plugin.imports(engine) {
        if name.starts_with("wasi:random/") {
            random(&mut linker.root(), name, &import.ty, engine)?;
        } else if !name.starts_with("noura:plugin/") {
            trap(&mut linker.root(), name, &import.ty, engine)?;
        }
    }
    Ok(())
}

/// The most random bytes one call hands a plugin.
const MAX_RANDOM_BYTES: u64 = 1 << 20;

/// Serves the WASI random interfaces from the operating system's random
/// source, so a plugin's `HashMap` seeds itself and libraries such as
/// minijinja run. Noura hands out no other WASI interface; a function of
/// these it doesn't know traps like any other.
fn random<T: 'static>(
    linker: &mut LinkerInstance<'_, T>,
    name: &str,
    item: &ComponentItem,
    engine: &Engine,
) -> wasmtime::Result<()> {
    let ComponentItem::ComponentInstance(instance) = item else {
        return trap(linker, name, item, engine);
    };
    let mut inner = linker.instance(name)?;
    for (export, item) in instance.exports(engine) {
        match export {
            "insecure-seed" => inner.func_wrap("insecure-seed", |_, (): ()| {
                Ok(((rand::random::<u64>(), rand::random::<u64>()),))
            })?,
            "get-random-u64" | "get-insecure-random-u64" => {
                inner.func_wrap(export, |_, (): ()| Ok((rand::random::<u64>(),)))?;
            }
            "get-random-bytes" | "get-insecure-random-bytes" => {
                inner.func_wrap(export, |_, (length,): (u64,)| {
                    let length = length.min(MAX_RANDOM_BYTES) as usize;
                    let mut bytes = vec![0u8; length];
                    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
                    Ok((bytes,))
                })?;
            }
            other => trap(&mut inner, other, &item.ty, engine)?,
        }
    }
    Ok(())
}

fn trap<T: 'static>(
    linker: &mut LinkerInstance<'_, T>,
    name: &str,
    item: &ComponentItem,
    engine: &Engine,
) -> wasmtime::Result<()> {
    match item {
        ComponentItem::ComponentFunc(_) => {
            let full = name.to_owned();
            linker.func_new(name, move |_, _, _, _| {
                wasmtime::bail!("the plugin called `{full}`, which Noura doesn't provide")
            })
        }
        ComponentItem::ComponentInstance(instance) => {
            let mut inner = linker.instance(name)?;
            for (export, item) in instance.exports(engine) {
                trap(&mut inner, export, &item.ty, engine)?;
            }
            Ok(())
        }
        ComponentItem::Resource(_) => {
            linker.resource(name, ResourceType::host::<()>(), |_, _| Ok(()))
        }
        _ => Ok(()),
    }
}

/// What registering one of a plugin's types gave: the type's name and
/// whether it is the plugin's now.
pub type Registration = (String, Result<(), TypeError>);

/// One store of a running plugin. It keeps the host's epoch ticker
/// running, so its call deadline holds after the host is dropped.
pub(crate) struct PluginInstance {
    store: Store<HostState>,
    exports: Exports,
    limits: Limits,
    _epochs: Arc<EpochTicker>,
}

/// The running plugin a store belongs to: its ID, and the token that
/// holds the object types it registers.
pub(crate) struct Owner<'a> {
    pub(crate) plugin: &'a str,
    pub(crate) token: u64,
}

impl PluginInstance {
    /// Start `component` in a new store for `owner` that may use what
    /// `grants` holds.
    pub(crate) fn start(
        engine: &Engine,
        epochs: &Arc<EpochTicker>,
        limits: Limits,
        component: &Component,
        owner: Owner,
        grants: Grants,
        services: Services,
    ) -> Result<Self, PluginError> {
        let state = HostState {
            limiter: StoreLimiter::new(limits.memory_bytes),
            grants,
            services,
            chain: Vec::new(),
            clock: Clock::start(limits),
            plugin: owner.plugin.to_owned(),
            instance: owner.token,
        };
        let mut store = Store::new(engine, state);
        store.limiter(|state| &mut state.limiter);
        store.set_epoch_deadline(deadline_ticks(limits.call_deadline));
        // The deadline counts the plugin's own work: when it passes, time
        // spent waiting for the network or another plugin buys more ticks.
        store.epoch_deadline_callback(|mut store| {
            let clock = &mut store.data_mut().clock;
            match clock.passed() {
                Some(limit) => {
                    clock.passed = Some(limit);
                    Ok(UpdateDeadline::Interrupt)
                }
                None => Ok(UpdateDeadline::Continue(deadline_ticks(
                    clock.deadline - clock.used(),
                ))),
            }
        });
        let mut linker = Linker::new(engine);
        Plugin::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |state| state)
            .map_err(|error| PluginError::Instantiate(format!("{error:?}")))?;
        trap_foreign_imports(&mut linker, component)
            .map_err(|error| PluginError::Instantiate(format!("{error:?}")))?;
        let exports = (|| -> wasmtime::Result<Exports> {
            let pre = linker.instantiate_pre(component)?;
            let instance = pre.instantiate(&mut store)?;
            let store = &mut store;
            Ok(Exports {
                events: export!(
                    events_export,
                    "noura:plugin/kernel-events@0.1.0",
                    &pre,
                    store,
                    &instance
                ),
                view: export!(
                    view_export,
                    "noura:plugin/view@0.1.0",
                    &pre,
                    store,
                    &instance
                ),
                object_types: export!(
                    types_export,
                    "noura:plugin/object-types@0.1.0",
                    &pre,
                    store,
                    &instance
                ),
                commands: export!(
                    commands_export,
                    "noura:plugin/commands@0.1.0",
                    &pre,
                    store,
                    &instance
                ),
                inbox: export!(
                    inbox_export,
                    "noura:plugin/inbox@0.1.0",
                    &pre,
                    store,
                    &instance
                ),
                settings: export!(
                    settings_export,
                    "noura:plugin/settings@0.1.0",
                    &pre,
                    store,
                    &instance
                ),
                tools: export!(
                    tools_export,
                    "noura:plugin/tools@0.1.0",
                    &pre,
                    store,
                    &instance
                ),
            })
        })()
        .map_err(|error| failure(&store, &error, PluginError::Instantiate))?;
        Ok(Self {
            store,
            exports,
            limits,
            _epochs: Arc::clone(epochs),
        })
    }

    /// Start a call made on behalf of `chain`, the plugins waiting for it:
    /// a fresh deadline, and no memory-cap refusal left from an earlier
    /// call, so a failure reports its own cause.
    pub(crate) fn begin(&mut self, chain: Vec<String>) {
        let state = self.store.data_mut();
        state.chain = chain;
        state.clock = Clock::start(self.limits);
        state.limiter.cap_hit = false;
        self.store
            .set_epoch_deadline(deadline_ticks(self.limits.call_deadline));
    }

    /// Fails unless the plugin holds `capability`.
    fn needs(&self, capability: &'static str) -> Result<(), PluginError> {
        if self.store.data().holds(capability) {
            Ok(())
        } else {
            Err(PluginError::CapabilityNotDeclared { capability })
        }
    }

    fn failed(&self, error: &wasmtime::Error) -> PluginError {
        failure(&self.store, error, PluginError::Call)
    }

    fn view(&self) -> Result<view_export::Guest, PluginError> {
        self.needs("views")?;
        self.exports.view.clone().ok_or(PluginError::NotExported {
            interface: "noura:plugin/view",
        })
    }

    fn commands_export(&self) -> Result<commands_export::Guest, PluginError> {
        self.needs("commands")?;
        self.exports
            .commands
            .clone()
            .ok_or(PluginError::NotExported {
                interface: "noura:plugin/commands",
            })
    }

    fn inbox(&self) -> Result<inbox_export::Guest, PluginError> {
        self.needs("inbox-sections")?;
        self.exports.inbox.clone().ok_or(PluginError::NotExported {
            interface: "noura:plugin/inbox",
        })
    }

    fn settings(&self) -> Result<settings_export::Guest, PluginError> {
        self.needs("settings")?;
        self.exports
            .settings
            .clone()
            .ok_or(PluginError::NotExported {
                interface: "noura:plugin/settings",
            })
    }

    /// Hand the plugin `event`, when it holds the capability the event
    /// needs: `global-shortcuts` for a shortcut, `menu-bar-items` for a
    /// menu entry, `notifications` for a notification and `speech` for
    /// the microphone and recordings.
    pub(crate) fn handle_kernel_event(&mut self, event: &KernelEvent) -> Result<(), PluginError> {
        self.needs(match event {
            KernelEvent::ShortcutPressed(_) | KernelEvent::ShortcutReleased(_) => {
                "global-shortcuts"
            }
            KernelEvent::MenuItemChosen(_) => "menu-bar-items",
            KernelEvent::NotificationChosen(_) => "notifications",
            KernelEvent::MicrophoneInUse(_)
            | KernelEvent::MicrophoneFree
            | KernelEvent::RecordingChanged(_) => "speech",
        })?;
        let events = self
            .exports
            .events
            .clone()
            .ok_or(PluginError::NotExported {
                interface: "noura:plugin/kernel-events",
            })?;
        events
            .call_handle_kernel_event(&mut self.store, event)
            .map_err(|error| self.failed(&error))
    }

    fn tools_export(&self) -> Result<tools_export::Guest, PluginError> {
        self.needs("tools")?;
        self.exports.tools.clone().ok_or(PluginError::NotExported {
            interface: "noura:plugin/tools",
        })
    }

    pub(crate) fn pages(&mut self) -> Result<Vec<Page>, PluginError> {
        let view = self.view()?;
        view.call_pages(&mut self.store)
            .map_err(|error| self.failed(&error))
    }

    pub(crate) fn render(&mut self, page: &str) -> Result<Vec<Node>, PluginError> {
        let view = self.view()?;
        view.call_render(&mut self.store, page)
            .map_err(|error| self.failed(&error))
    }

    /// The nodes of the section the page `page` adds to the sidebar.
    pub(crate) fn render_sidebar(&mut self, page: &str) -> Result<Vec<Node>, PluginError> {
        let view = self.view()?;
        view.call_sidebar(&mut self.store, page)
            .map_err(|error| self.failed(&error))
    }

    /// Rows `start` up to `start + count` of the virtual list `node` in a
    /// page's view, as the plugin returned them.
    pub(crate) fn rows(
        &mut self,
        page: &str,
        node: &str,
        start: u32,
        count: u32,
    ) -> Result<Vec<VirtualRow>, PluginError> {
        let view = self.view()?;
        view.call_rows(&mut self.store, page, node, start, count)
            .map_err(|error| self.failed(&error))
    }

    pub(crate) fn handle_event(&mut self, page: &str, event: &Event) -> Result<(), PluginError> {
        let view = self.view()?;
        view.call_handle_event(&mut self.store, page, event)
            .map_err(|error| self.failed(&error))
    }

    pub(crate) fn commands(&mut self) -> Result<Vec<Command>, PluginError> {
        let commands = self.commands_export()?;
        commands
            .call_commands(&mut self.store)
            .map_err(|error| self.failed(&error))
    }

    pub(crate) fn run_command(
        &mut self,
        id: &str,
        input: &str,
    ) -> Result<Result<String, String>, PluginError> {
        let commands = self.commands_export()?;
        commands
            .call_run(&mut self.store, id, input)
            .map_err(|error| self.failed(&error))
    }

    pub(crate) fn inbox_sections(&mut self) -> Result<Vec<InboxSection>, PluginError> {
        let inbox = self.inbox()?;
        inbox
            .call_sections(&mut self.store)
            .map_err(|error| self.failed(&error))
    }

    pub(crate) fn render_inbox_section(&mut self, section: &str) -> Result<Vec<Node>, PluginError> {
        let inbox = self.inbox()?;
        inbox
            .call_render_section(&mut self.store, section)
            .map_err(|error| self.failed(&error))
    }

    pub(crate) fn handle_inbox_event(
        &mut self,
        section: &str,
        event: &Event,
    ) -> Result<(), PluginError> {
        let inbox = self.inbox()?;
        inbox
            .call_handle_section_event(&mut self.store, section, event)
            .map_err(|error| self.failed(&error))
    }

    pub(crate) fn settings_page(&mut self) -> Result<Option<String>, PluginError> {
        let settings = self.settings()?;
        settings
            .call_page(&mut self.store)
            .map_err(|error| self.failed(&error))
    }

    pub(crate) fn render_settings(&mut self) -> Result<Vec<Node>, PluginError> {
        let settings = self.settings()?;
        settings
            .call_render_settings(&mut self.store)
            .map_err(|error| self.failed(&error))
    }

    pub(crate) fn handle_settings_event(&mut self, event: &Event) -> Result<(), PluginError> {
        let settings = self.settings()?;
        settings
            .call_handle_settings_event(&mut self.store, event)
            .map_err(|error| self.failed(&error))
    }

    pub(crate) fn tools(&mut self) -> Result<Vec<Tool>, PluginError> {
        let tools = self.tools_export()?;
        tools
            .call_tools(&mut self.store)
            .map_err(|error| self.failed(&error))
    }

    pub(crate) fn call_tool(
        &mut self,
        name: &str,
        arguments: &str,
    ) -> Result<Result<String, String>, PluginError> {
        let tools = self.tools_export()?;
        tools
            .call_call_tool(&mut self.store, name, arguments)
            .map_err(|error| self.failed(&error))
    }

    /// Ask the plugin for its object types and register each one in the
    /// workspace's registry for its running plugin. Each type comes back
    /// with whether it registered: the kernel's own types, a type another
    /// plugin holds and an invalid schema each fail with a structured
    /// error, and the other types register all the same.
    pub(crate) fn register_object_types(&mut self) -> Result<Vec<Registration>, PluginError> {
        let export = self.object_types_export()?;
        let declared = export
            .call_types(&mut self.store)
            .map_err(|error| self.failed(&error))?;
        let state = self.store.data();
        Ok(declared
            .into_iter()
            .map(|declared| {
                let registered = state.services.types.register(
                    &state.plugin,
                    state.instance,
                    &declared.name,
                    &declared.schema,
                );
                (declared.name, registered)
            })
            .collect())
    }

    pub(crate) fn render_panel(&mut self, object: &str) -> Result<Vec<Node>, PluginError> {
        let export = self.object_types_export()?;
        export
            .call_render_panel(&mut self.store, object)
            .map_err(|error| self.failed(&error))
    }

    pub(crate) fn handle_panel_event(
        &mut self,
        object: &str,
        event: &Event,
    ) -> Result<(), PluginError> {
        let export = self.object_types_export()?;
        export
            .call_handle_panel_event(&mut self.store, object, event)
            .map_err(|error| self.failed(&error))
    }

    fn object_types_export(&self) -> Result<types_export::Guest, PluginError> {
        self.needs(OBJECT_TYPES)?;
        self.exports
            .object_types
            .clone()
            .ok_or(PluginError::NotExported {
                interface: "noura:plugin/object-types",
            })
    }
}

fn failure(
    store: &Store<HostState>,
    error: &wasmtime::Error,
    other: fn(String) -> PluginError,
) -> PluginError {
    let clock = store.data().clock;
    match Failure::of(&store.data().limiter, error) {
        Failure::Deadline => PluginError::Deadline {
            deadline: clock.passed.unwrap_or(clock.deadline),
        },
        Failure::MemoryCap { cap_bytes } => PluginError::MemoryCap { cap_bytes },
        Failure::Other(message) => other(message),
    }
}
