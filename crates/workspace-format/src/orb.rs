//! Orbs: personal agents that live in the workspace as files. An Orb is a
//! managed Markdown object of type `orb` whose body holds its instructions
//! and whose frontmatter holds its look, its provider, the tools it runs,
//! what it reads, its memory and its triggers. Each run of an Orb leaves an
//! `orb-run` record beside it.
//!
//! This module holds the parts of the format that need no filesystem: the
//! two schemas, the rules an Orb's properties follow beyond them, the
//! trigger grammar, schedule times, trigger keys and the layout of an Orb's
//! folder. The folder borrows OpenClaw's agent workspace conventions: the
//! instructions file, `Memory.md` for what the Orb keeps, and
//! `Heartbeat.md` for the checklist its scheduled runs follow. See
//! `docs/workspace-format/v1.md` § Orbs.

use std::{collections::BTreeSet, str::FromStr};

use croner::{Cron, parser::CronParser, parser::Seconds};
use jiff::{Timestamp, Zoned, tz::TimeZone};
use serde_json::Value;

use crate::{
    ObjectTypeRegistry, PropertyError, PropertySchema, WorkspaceObject, normalize_file_name,
    valid_object_id, valid_plugin_id,
};

/// The type of an Orb's own file.
pub const ORB_TYPE: &str = "orb";

/// The type of a record of one run of an Orb.
pub const ORB_RUN_TYPE: &str = "orb-run";

/// The folder Noura creates Orbs in. Each Orb gets a folder of its own
/// inside it, named after the Orb.
pub const ORBS_FOLDER: &str = "Orbs";

/// The file beside an Orb that holds what it remembers, as OpenClaw's
/// `MEMORY.md` does.
pub const MEMORY_FILE: &str = "Memory.md";

/// The file beside an Orb that holds the checklist its scheduled runs
/// follow, as OpenClaw's `HEARTBEAT.md` does.
pub const HEARTBEAT_FILE: &str = "Heartbeat.md";

/// The folder beside an Orb that holds its run records, one folder per
/// day.
pub const RUNS_FOLDER: &str = "Runs";

/// What an Orb answers when a scheduled run finds nothing to report, as
/// OpenClaw's heartbeat does. A run whose last answer is only this is
/// quiet.
pub const HEARTBEAT_OK: &str = "HEARTBEAT_OK";

/// The Orb that a chat without `orb_id` belongs to: plain chats from
/// before Orbs, and every chat you start outside one. It has no file.
pub const DEFAULT_ORB_ID: &str = "orb_00000000000000000000000000";

/// The fewest minutes between two times of a schedule trigger.
pub const MIN_SCHEDULE_MINUTES: i64 = 5;

/// The longest tool name providers take.
const MAX_TOOL_NAME: usize = 64;

/// Between a plugin's ID and its tool's name in a tool name.
const TOOL_SEPARATOR: &str = "__";

/// The longest avatar, in characters: an emoji or a few letters.
const MAX_AVATAR_CHARS: usize = 16;

/// The colors an Orb takes, Noura's palette.
pub const ORB_COLORS: [&str; 8] = [
    "red", "orange", "yellow", "green", "blue", "purple", "brown", "gray",
];

/// The schema of an Orb, in canonical order.
pub const ORB_SCHEMA: &str = r#"{"properties":[
  {"name":"avatar","kind":"string","nullable":true},
  {"name":"color","kind":{"enum":["red","orange","yellow","green","blue","purple","brown","gray"]},"nullable":true},
  {"name":"provider","kind":"string","nullable":true},
  {"name":"model","kind":"string","nullable":true},
  {"name":"tools","kind":{"list":"string"},"default":[]},
  {"name":"reads","kind":{"list":{"reference":"*"}},"default":[]},
  {"name":"memory","kind":{"reference":"*"},"nullable":true},
  {"name":"triggers","kind":{"list":"string"},"default":[]},
  {"name":"paused","kind":"boolean","default":false}
]}"#;

/// The schema of a run record, in canonical order.
pub const ORB_RUN_SCHEMA: &str = r#"{"properties":[
  {"name":"orb_id","kind":{"id":"orb"}},
  {"name":"chat_id","kind":{"id":"chat"}},
  {"name":"started_by","kind":{"enum":["you","schedule","event"]}},
  {"name":"trigger","kind":"string","nullable":true},
  {"name":"trigger_key","kind":"string","nullable":true},
  {"name":"object_id","kind":"string","nullable":true},
  {"name":"slot","kind":"date","nullable":true},
  {"name":"status","kind":{"enum":["running","succeeded","failed","interrupted","cancelled"]}},
  {"name":"error_code","kind":"string","nullable":true},
  {"name":"tool_calls","kind":"number","default":0},
  {"name":"quiet","kind":"boolean","default":false},
  {"name":"resumed","kind":"number","default":0},
  {"name":"finished","kind":"date","nullable":true}
]}"#;

/// The schema of an Orb.
pub fn orb_schema() -> PropertySchema {
    serde_json::from_str(ORB_SCHEMA).expect("the Orb schema is a schema declaration")
}

/// The schema of a run record.
pub fn orb_run_schema() -> PropertySchema {
    serde_json::from_str(ORB_RUN_SCHEMA).expect("the run schema is a schema declaration")
}

/// A registry that holds the Orb and run record schemas, for the service
/// that writes them whether or not the plugin that registers them runs.
pub fn orb_registry() -> ObjectTypeRegistry {
    let mut registry = ObjectTypeRegistry::new();
    registry
        .register(ORB_TYPE, orb_schema())
        .expect("the Orb schema registers");
    registry
        .register(ORB_RUN_TYPE, orb_run_schema())
        .expect("the run schema registers");
    registry
}

/// An Orb as its file holds it, checked.
#[derive(Debug, Clone, PartialEq)]
pub struct Orb {
    pub id: String,
    /// Its name: the file's name without `.md`.
    pub title: String,
    pub relative_path: String,
    pub revision: String,
    /// The file's body: what the Orb does, in your words.
    pub instructions: String,
    pub avatar: Option<String>,
    pub color: Option<String>,
    /// A provider ID on this device, or none for your first provider.
    pub provider: Option<String>,
    /// A model in place of the provider's own, or none.
    pub model: Option<String>,
    /// The tools it runs without asking.
    pub tools: Vec<ToolPattern>,
    /// The links its read tool and its trigger messages may follow. Empty
    /// means the whole workspace.
    pub reads: Vec<String>,
    /// The link to its memory file, between `[[` and `]]`.
    pub memory: Option<String>,
    pub triggers: Vec<Trigger>,
    /// You paused it: none of its triggers fire.
    pub paused: bool,
}

/// Why an Orb's file doesn't make an Orb, naming the first property that
/// fails, in declared order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrbProblem {
    pub property: String,
    /// A schema code (`wrong_type`, `not_allowed`, `invalid_date`,
    /// `invalid_reference`) or an Orb rule's code: `invalid_avatar`,
    /// `invalid_provider`, `invalid_model`, `invalid_tool`,
    /// `duplicate_tool`, `invalid_trigger`, `schedule_too_frequent` or
    /// `duplicate_trigger`.
    pub code: String,
}

impl OrbProblem {
    fn new(property: &str, code: &str) -> Self {
        Self {
            property: property.to_owned(),
            code: code.to_owned(),
        }
    }
}

impl std::fmt::Display for OrbProblem {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.property, self.code)
    }
}

impl From<PropertyError> for OrbProblem {
    fn from(error: PropertyError) -> Self {
        let (property, code) = match error {
            PropertyError::WrongType { property } => (property, "wrong_type"),
            PropertyError::NotAllowed { property } => (property, "not_allowed"),
            PropertyError::InvalidDate { property } => (property, "invalid_date"),
            PropertyError::InvalidReference { property } => (property, "invalid_reference"),
        };
        Self {
            property,
            code: code.to_owned(),
        }
    }
}

/// Read the Orb `object` holds: its properties pass the Orb schema, which
/// adds the defaults they lack, then the Orb rules. Unknown properties are
/// left alone.
pub fn read_orb(object: &WorkspaceObject) -> Result<Orb, OrbProblem> {
    if object.object_type != ORB_TYPE || !valid_object_id(&object.id, ORB_TYPE) {
        return Err(OrbProblem::new("type", "wrong_type"));
    }
    let mut checked = object.clone();
    orb_registry().normalize(&mut checked)?;
    let properties = &checked.properties;
    let avatar = optional_string(properties.get("avatar"));
    if let Some(avatar) = &avatar
        && (avatar.trim().is_empty()
            || avatar.chars().count() > MAX_AVATAR_CHARS
            || avatar.chars().any(char::is_control))
    {
        return Err(OrbProblem::new("avatar", "invalid_avatar"));
    }
    let color = optional_string(properties.get("color"));
    let provider = optional_string(properties.get("provider"));
    if provider.as_deref().is_some_and(|value| !single_line(value)) {
        return Err(OrbProblem::new("provider", "invalid_provider"));
    }
    let model = optional_string(properties.get("model"));
    if model.as_deref().is_some_and(|value| !single_line(value)) {
        return Err(OrbProblem::new("model", "invalid_model"));
    }
    let mut tools = Vec::new();
    for name in strings(properties.get("tools")) {
        let pattern =
            ToolPattern::parse(&name).ok_or_else(|| OrbProblem::new("tools", "invalid_tool"))?;
        if tools.contains(&pattern) {
            return Err(OrbProblem::new("tools", "duplicate_tool"));
        }
        tools.push(pattern);
    }
    let reads = strings(properties.get("reads"))
        .iter()
        .filter_map(|value| crate::links::property_link(value).map(str::to_owned))
        .collect();
    let memory = properties
        .get("memory")
        .and_then(Value::as_str)
        .and_then(crate::links::property_link)
        .map(str::to_owned);
    let mut triggers = Vec::new();
    let mut seen = BTreeSet::new();
    for text in strings(properties.get("triggers")) {
        let trigger = Trigger::parse(&text).map_err(|code| OrbProblem::new("triggers", code))?;
        if !seen.insert(trigger.canonical()) {
            return Err(OrbProblem::new("triggers", "duplicate_trigger"));
        }
        triggers.push(trigger);
    }
    let paused = properties
        .get("paused")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(Orb {
        id: object.id.clone(),
        title: object.title.clone(),
        relative_path: object.relative_path.clone(),
        revision: object.revision.clone(),
        instructions: object.body.clone(),
        avatar,
        color,
        provider,
        model,
        tools,
        reads,
        memory,
        triggers,
        paused,
    })
}

fn optional_string(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_owned)
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn single_line(value: &str) -> bool {
    !value.trim().is_empty() && !value.chars().any(char::is_control)
}

/// A tool an Orb runs: one tool, `<plugin>__<tool>`, or every tool of a
/// plugin, `<plugin>__*`. Noura's own tools are the plugin `noura`'s.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ToolPattern {
    Tool { plugin: String, tool: String },
    Plugin(String),
}

impl ToolPattern {
    /// The pattern `text` names, or none when it isn't one.
    pub fn parse(text: &str) -> Option<Self> {
        let (plugin, tool) = text.split_once(TOOL_SEPARATOR)?;
        if !valid_plugin_id(plugin) {
            return None;
        }
        if tool == "*" {
            return Some(Self::Plugin(plugin.to_owned()));
        }
        let valid_tool = text.len() <= MAX_TOOL_NAME
            && tool.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
            && tool.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
            });
        valid_tool.then(|| Self::Tool {
            plugin: plugin.to_owned(),
            tool: tool.to_owned(),
        })
    }

    /// Whether the tool the model sees as `name` falls under the pattern.
    pub fn matches(&self, name: &str) -> bool {
        let Some((plugin, tool)) = name.split_once(TOOL_SEPARATOR) else {
            return false;
        };
        match self {
            Self::Tool {
                plugin: known,
                tool: named,
            } => known == plugin && named == tool,
            Self::Plugin(known) => known == plugin && !tool.is_empty(),
        }
    }
}

impl std::fmt::Display for ToolPattern {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tool { plugin, tool } => write!(formatter, "{plugin}{TOOL_SEPARATOR}{tool}"),
            Self::Plugin(plugin) => write!(formatter, "{plugin}{TOOL_SEPARATOR}*"),
        }
    }
}

/// What starts an Orb's run without you.
#[derive(Debug, Clone)]
pub enum Trigger {
    /// The times a 5-field cron pattern names, in this device's time zone,
    /// written `schedule 30 8 * * 1-5`.
    Schedule(Schedule),
    /// A plugin told Noura about an event, written `event
    /// transcription.finished`.
    Event(EventName),
}

impl PartialEq for Trigger {
    fn eq(&self, other: &Self) -> bool {
        self.canonical() == other.canonical()
    }
}

impl Trigger {
    /// The trigger `text` names. Fails with `invalid_trigger`, or with
    /// `schedule_too_frequent` for a schedule whose times come less than
    /// [`MIN_SCHEDULE_MINUTES`] apart.
    pub fn parse(text: &str) -> Result<Self, &'static str> {
        let text = text.trim();
        let (keyword, rest) = text
            .split_once(char::is_whitespace)
            .ok_or("invalid_trigger")?;
        match keyword {
            "schedule" => Schedule::parse(rest).map(Self::Schedule),
            "event" => EventName::parse(rest.trim())
                .map(Self::Event)
                .ok_or("invalid_trigger"),
            _ => Err("invalid_trigger"),
        }
    }

    /// The trigger as Noura writes and compares it: the keyword, one
    /// space, then the schedule's fields one space apart or the event's
    /// name.
    pub fn canonical(&self) -> String {
        match self {
            Self::Schedule(schedule) => format!("schedule {}", schedule.pattern()),
            Self::Event(event) => format!("event {event}"),
        }
    }
}

/// An event a plugin declares, named `<plugin>.<event>`, such as
/// `transcription.finished`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EventName {
    pub plugin: String,
    pub event: String,
}

impl EventName {
    /// The event `text` names, or none when it isn't one. Both parts use
    /// a plugin ID's characters: lowercase letters, digits and hyphens,
    /// starting with a letter.
    pub fn parse(text: &str) -> Option<Self> {
        let (plugin, event) = text.split_once('.')?;
        (valid_plugin_id(plugin) && valid_plugin_id(event)).then(|| Self {
            plugin: plugin.to_owned(),
            event: event.to_owned(),
        })
    }
}

impl std::fmt::Display for EventName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}.{}", self.plugin, self.event)
    }
}

/// A 5-field cron pattern: minute, hour, day of the month, month and day
/// of the week, with croner's syntax, such as `30 8 * * 1-5` or
/// `@daily`. Its times fall in this device's time zone.
#[derive(Clone)]
pub struct Schedule {
    pattern: String,
    cron: Cron,
}

impl std::fmt::Debug for Schedule {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("Schedule")
            .field(&self.pattern)
            .finish()
    }
}

impl Schedule {
    /// The schedule `text` names.
    pub fn parse(text: &str) -> Result<Self, &'static str> {
        let pattern = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if pattern.is_empty() || pattern.len() > 128 {
            return Err("invalid_trigger");
        }
        let cron = CronParser::builder()
            .seconds(Seconds::Disallowed)
            .build()
            .parse(&pattern)
            .map_err(|_| "invalid_trigger")?;
        let schedule = Self { pattern, cron };
        if !schedule.spaced() {
            return Err("schedule_too_frequent");
        }
        Ok(schedule)
    }

    /// The pattern, its fields one space apart.
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    /// Whether its times come at least [`MIN_SCHEDULE_MINUTES`] apart, as
    /// a year of them from 2026 in UTC shows. A pattern with no time at
    /// all in that year counts as spaced, and simply never fires.
    fn spaced(&self) -> bool {
        let Ok(start) = "2026-01-01T00:00:00Z"
            .parse::<Timestamp>()
            .map(|instant| instant.to_zoned(TimeZone::UTC))
        else {
            return false;
        };
        let mut times = self.cron.iter_after(start);
        let Some(mut previous) = times.next() else {
            return true;
        };
        // A pattern that fires at least every five minutes shows it within
        // its first few hundred times.
        for next in times.take(512) {
            if next.timestamp().as_second() - previous.timestamp().as_second()
                < MIN_SCHEDULE_MINUTES * 60
            {
                return false;
            }
            previous = next;
        }
        true
    }

    /// The first time after `after`, in `zone`.
    pub fn next_after(&self, after: Timestamp, zone: &TimeZone) -> Option<Timestamp> {
        self.cron
            .find_next_occurrence(&after.to_zoned(zone.clone()), false)
            .ok()
            .map(|time: Zoned| time.timestamp())
    }

    /// The last time at or before `now`, in `zone`, when it comes after
    /// `since`: the run a launch catches up on. Several missed times make
    /// one run.
    pub fn missed(&self, since: Timestamp, now: Timestamp, zone: &TimeZone) -> Option<Timestamp> {
        let latest = self
            .cron
            .find_previous_occurrence(&now.to_zoned(zone.clone()), true)
            .ok()?
            .timestamp();
        (latest > since).then_some(latest)
    }
}

impl FromStr for Schedule {
    type Err = &'static str;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

/// The key that names one firing of `trigger` of the Orb `orb`, so Noura
/// starts each firing once: a scheduled time, in UTC as RFC 3339, or an
/// event's object and its revision. Run records hold it.
pub fn trigger_key(orb: &str, trigger: &Trigger, occurrence: &str) -> String {
    format!("{orb}|{}|{occurrence}", trigger.canonical())
}

/// What started a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartedBy {
    /// You wrote to the Orb, or chose Run now.
    You,
    /// A schedule trigger's time came.
    Schedule,
    /// An event trigger's event happened.
    Event,
}

impl StartedBy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::You => "you",
            Self::Schedule => "schedule",
            Self::Event => "event",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "you" => Self::You,
            "schedule" => Self::Schedule,
            "event" => Self::Event,
            _ => return None,
        })
    }
}

/// How a run stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    /// It runs now, or Noura quit while it ran and hasn't looked since.
    Running,
    /// It finished with an answer.
    Succeeded,
    /// The provider or a write failed. `error_code` says why.
    Failed,
    /// Noura quit while it ran a tool, or a plugin it relied on went, so
    /// Noura didn't finish it. It never runs a tool call again by itself.
    Interrupted,
    /// You stopped it.
    Cancelled,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "running" => Self::Running,
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "interrupted" => Self::Interrupted,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }
}

/// A run record, checked.
#[derive(Debug, Clone, PartialEq)]
pub struct OrbRun {
    pub id: String,
    pub relative_path: String,
    pub revision: String,
    pub orb_id: String,
    /// The Orb's thread, where the run's messages are.
    pub chat_id: String,
    pub started_by: StartedBy,
    /// The trigger, as [`Trigger::canonical`] writes it, for a run a
    /// trigger started.
    pub trigger: Option<String>,
    pub trigger_key: Option<String>,
    /// The object an event named.
    pub object_id: Option<String>,
    /// The scheduled time a schedule trigger fired for.
    pub slot: Option<String>,
    pub status: RunStatus,
    pub error_code: Option<String>,
    pub tool_calls: u32,
    pub quiet: bool,
    /// How many times Noura took the run up again after a quit.
    pub resumed: u32,
    pub started: Option<String>,
    pub finished: Option<String>,
    /// One line per tool call the run made.
    pub log: String,
}

/// Read the run record `object` holds, or why it isn't one.
pub fn read_orb_run(object: &WorkspaceObject) -> Result<OrbRun, OrbProblem> {
    if object.object_type != ORB_RUN_TYPE || !valid_object_id(&object.id, ORB_RUN_TYPE) {
        return Err(OrbProblem::new("type", "wrong_type"));
    }
    let mut checked = object.clone();
    orb_registry().normalize(&mut checked)?;
    let properties = &checked.properties;
    let required_id = |name: &str, object_type: &str| {
        properties
            .get(name)
            .and_then(Value::as_str)
            .filter(|id| valid_object_id(id, object_type))
            .map(str::to_owned)
            .ok_or_else(|| OrbProblem::new(name, "invalid_reference"))
    };
    let orb_id = required_id("orb_id", ORB_TYPE)?;
    let chat_id = required_id("chat_id", "chat")?;
    let started_by = properties
        .get("started_by")
        .and_then(Value::as_str)
        .and_then(StartedBy::parse)
        .ok_or_else(|| OrbProblem::new("started_by", "wrong_type"))?;
    let status = properties
        .get("status")
        .and_then(Value::as_str)
        .and_then(RunStatus::parse)
        .ok_or_else(|| OrbProblem::new("status", "wrong_type"))?;
    let count = |name: &str| {
        properties
            .get(name)
            .and_then(Value::as_u64)
            .and_then(|count| u32::try_from(count).ok())
            .ok_or_else(|| OrbProblem::new(name, "wrong_type"))
    };
    Ok(OrbRun {
        id: object.id.clone(),
        relative_path: object.relative_path.clone(),
        revision: object.revision.clone(),
        orb_id,
        chat_id,
        started_by,
        trigger: optional_string(properties.get("trigger")),
        trigger_key: optional_string(properties.get("trigger_key")),
        object_id: optional_string(properties.get("object_id")),
        slot: optional_string(properties.get("slot")),
        status,
        error_code: optional_string(properties.get("error_code")),
        tool_calls: count("tool_calls")?,
        quiet: properties
            .get("quiet")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        resumed: count("resumed")?,
        started: object.created.clone(),
        finished: optional_string(properties.get("finished")),
        log: object.body.clone(),
    })
}

/// Whether `answer`, a run's last answer, says only that there is
/// nothing to report.
pub fn is_quiet(answer: &str) -> bool {
    answer.trim().trim_matches('*').trim() == HEARTBEAT_OK
}

/// The folder a new Orb named `title` gets: `Orbs/<title>`, with the
/// title made safe for a file name.
pub fn orb_folder(title: &str) -> String {
    let name = normalize_file_name(title);
    let name = if name.is_empty() { "Orb" } else { &name };
    format!("{ORBS_FOLDER}/{name}")
}

/// The folder that holds `orb_path`, the Orb's own file: where its memory,
/// its heartbeat checklist and its runs go.
pub fn orb_home(orb_path: &str) -> &str {
    orb_path.rsplit_once('/').map_or("", |(folder, _)| folder)
}

/// The path of the run record `id` of the Orb whose file is `orb_path`,
/// for a run that started on the civil date `date` (`YYYY-MM-DD`).
pub fn run_path(orb_path: &str, date: &str, id: &str) -> String {
    let home = orb_home(orb_path);
    if home.is_empty() {
        format!("{RUNS_FOLDER}/{date}/{id}.md")
    } else {
        format!("{home}/{RUNS_FOLDER}/{date}/{id}.md")
    }
}

/// The path beside the Orb's file of `name`, such as [`MEMORY_FILE`].
pub fn beside(orb_path: &str, name: &str) -> String {
    let home = orb_home(orb_path);
    if home.is_empty() {
        name.to_owned()
    } else {
        format!("{home}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn orb(properties: Value) -> WorkspaceObject {
        WorkspaceObject {
            id: "orb_01jd8x3k9q2m7v4c6b1n0p5r8t".into(),
            object_type: ORB_TYPE.into(),
            title: "Meetings".into(),
            body: "You turn meetings into tasks.\n".into(),
            relative_path: "Orbs/Meetings/Meetings.md".into(),
            revision: "r".into(),
            created: None,
            updated: None,
            properties: serde_json::from_value(properties).unwrap(),
        }
    }

    #[test]
    fn an_orb_reads_with_its_defaults() {
        let read = read_orb(&orb(serde_json::json!({}))).unwrap();
        assert!(read.tools.is_empty());
        assert!(read.triggers.is_empty());
        assert!(!read.paused);
        assert_eq!(read.instructions, "You turn meetings into tasks.\n");
    }

    #[test]
    fn tools_and_triggers_follow_their_grammar() {
        let read = read_orb(&orb(serde_json::json!({
            "tools": ["tasks__create", "noura__*"],
            "triggers": ["event transcription.finished", "schedule  30 8 * * 1-5"],
            "memory": "[[Memory]]",
        })))
        .unwrap();
        assert!(read.tools[0].matches("tasks__create"));
        assert!(!read.tools[0].matches("tasks__list"));
        assert!(read.tools[1].matches("noura__remember"));
        assert_eq!(read.triggers[1].canonical(), "schedule 30 8 * * 1-5");
        assert_eq!(read.memory.as_deref(), Some("Memory"));
        for (properties, code) in [
            (serde_json::json!({"tools": ["tasks"]}), "invalid_tool"),
            (
                serde_json::json!({"tools": ["tasks__create", "tasks__create"]}),
                "duplicate_tool",
            ),
            (
                serde_json::json!({"triggers": ["schedule * * * * *"]}),
                "schedule_too_frequent",
            ),
            (
                serde_json::json!({"triggers": ["schedule 61 * * * *"]}),
                "invalid_trigger",
            ),
            (
                serde_json::json!({"triggers": ["event Transcription.finished"]}),
                "invalid_trigger",
            ),
            (
                serde_json::json!({"triggers": ["every day"]}),
                "invalid_trigger",
            ),
            (
                serde_json::json!({"triggers": ["schedule @daily", "schedule  @daily"]}),
                "duplicate_trigger",
            ),
            (serde_json::json!({"paused": "no"}), "wrong_type"),
            (serde_json::json!({"color": "teal"}), "not_allowed"),
        ] {
            assert_eq!(
                read_orb(&orb(properties.clone())).unwrap_err().code,
                code,
                "{properties}"
            );
        }
    }

    #[test]
    fn a_schedule_finds_its_next_and_missed_times_in_the_device_zone() {
        let schedule = Schedule::parse("30 8 * * 1-5").unwrap();
        let zone = TimeZone::get("America/Toronto").unwrap();
        // Saturday 21 November 2026, 13:00 in Toronto.
        let now: Timestamp = "2026-11-21T18:00:00Z".parse().unwrap();
        assert_eq!(
            schedule.next_after(now, &zone).unwrap().to_string(),
            "2026-11-23T13:30:00Z"
        );
        let since: Timestamp = "2026-11-18T00:00:00Z".parse().unwrap();
        assert_eq!(
            schedule.missed(since, now, &zone).unwrap().to_string(),
            "2026-11-20T13:30:00Z"
        );
        let later: Timestamp = "2026-11-20T13:30:00Z".parse().unwrap();
        assert_eq!(schedule.missed(later, now, &zone), None);
    }

    #[test]
    fn a_quiet_answer_is_only_the_token() {
        assert!(is_quiet(" HEARTBEAT_OK\n"));
        assert!(is_quiet("**HEARTBEAT_OK**"));
        assert!(!is_quiet("HEARTBEAT_OK, and I made a task"));
    }

    #[test]
    fn an_orb_folder_holds_its_runs() {
        assert_eq!(orb_folder("Meetings"), "Orbs/Meetings");
        assert_eq!(orb_folder("a/b"), "Orbs/ab");
        assert_eq!(
            run_path("Orbs/Meetings/Meetings.md", "2026-11-21", "orb-run_1"),
            "Orbs/Meetings/Runs/2026-11-21/orb-run_1.md"
        );
        assert_eq!(beside("Meetings.md", MEMORY_FILE), "Memory.md");
    }
}

/// The shared fixture `docs/workspace-format/fixtures/orb-v1.json`, which
/// every reader of Orb files must pass.
#[cfg(test)]
mod conformance {
    use super::*;
    use crate::{ParsedMarkdown, parse_markdown};

    fn fixture() -> Value {
        serde_json::from_str(include_str!(
            "../../../docs/workspace-format/fixtures/orb-v1.json"
        ))
        .unwrap()
    }

    fn cases<'a>(fixture: &'a Value, section: &str) -> &'a [Value] {
        fixture[section].as_array().unwrap()
    }

    fn managed(case: &Value) -> WorkspaceObject {
        let path = case["path"].as_str().unwrap();
        match parse_markdown(path, case["markdown"].as_str().unwrap().as_bytes()) {
            ParsedMarkdown::Managed(object) => object,
            other => panic!("{}: {other:?}", case["name"]),
        }
    }

    fn instant(value: &Value) -> Timestamp {
        value.as_str().unwrap().parse().unwrap()
    }

    #[test]
    fn the_schemas_match_the_fixture() {
        let fixture = fixture();
        let orb: PropertySchema =
            serde_json::from_value(fixture["schemas"]["orb"].clone()).unwrap();
        let run: PropertySchema =
            serde_json::from_value(fixture["schemas"]["orb-run"].clone()).unwrap();
        assert_eq!(orb, orb_schema());
        assert_eq!(run, orb_run_schema());
    }

    #[test]
    fn each_orb_reads_or_names_its_problem() {
        for case in cases(&fixture(), "orbs") {
            let name = &case["name"];
            let read = read_orb(&managed(case));
            if case["valid"] == true {
                let orb = read.unwrap_or_else(|problem| panic!("{name}: {problem}"));
                let expected = &case["orb"];
                let tools: Vec<String> = orb.tools.iter().map(ToString::to_string).collect();
                let triggers: Vec<String> = orb.triggers.iter().map(Trigger::canonical).collect();
                let actual = serde_json::json!({
                    "title": orb.title,
                    "instructions": orb.instructions,
                    "avatar": orb.avatar,
                    "color": orb.color,
                    "tools": tools,
                    "reads": orb.reads,
                    "memory": orb.memory,
                    "triggers": triggers,
                    "paused": orb.paused,
                });
                assert_eq!(&actual, expected, "{name}");
            } else {
                let problem = read.expect_err(&name.to_string());
                assert_eq!(problem.property, case["problem"]["property"], "{name}");
                assert_eq!(problem.code, case["problem"]["code"], "{name}");
            }
        }
    }

    #[test]
    fn each_trigger_reads_as_its_canonical_text_or_fails() {
        for case in cases(&fixture(), "triggers") {
            let text = case["text"].as_str().unwrap();
            match (Trigger::parse(text), case.get("canonical")) {
                (Ok(trigger), Some(canonical)) => {
                    assert_eq!(trigger.canonical(), *canonical, "{text}")
                }
                (Err(code), None) => assert_eq!(code, case["error"], "{text}"),
                (read, _) => panic!("{text}: {read:?}"),
            }
        }
    }

    #[test]
    fn each_schedule_finds_its_times() {
        for case in cases(&fixture(), "schedules") {
            let name = &case["name"];
            let schedule = Schedule::parse(case["pattern"].as_str().unwrap()).unwrap();
            let zone = TimeZone::get(case["zone"].as_str().unwrap()).unwrap();
            if let Some(next) = case.get("next") {
                let found = schedule.next_after(instant(&case["after"]), &zone);
                assert_eq!(
                    found.map(|time| time.to_string()).as_deref(),
                    next.as_str(),
                    "{name}"
                );
            } else {
                let found = schedule.missed(instant(&case["since"]), instant(&case["now"]), &zone);
                assert_eq!(
                    found.map(|time| time.to_string()).as_deref(),
                    case["missed"].as_str(),
                    "{name}"
                );
            }
        }
    }

    #[test]
    fn each_trigger_key_names_one_firing() {
        for case in cases(&fixture(), "trigger_keys") {
            let trigger = Trigger::parse(case["trigger"].as_str().unwrap()).unwrap();
            let key = trigger_key(
                case["orb"].as_str().unwrap(),
                &trigger,
                case["occurrence"].as_str().unwrap(),
            );
            assert_eq!(key, case["key"].as_str().unwrap());
        }
    }

    #[test]
    fn each_run_record_reads_or_names_its_problem() {
        for case in cases(&fixture(), "runs") {
            let name = &case["name"];
            let read = read_orb_run(&managed(case));
            if case["valid"] == true {
                let run = read.unwrap_or_else(|problem| panic!("{name}: {problem}"));
                let actual = serde_json::json!({
                    "orb_id": run.orb_id,
                    "chat_id": run.chat_id,
                    "started_by": run.started_by.as_str(),
                    "status": run.status.as_str(),
                    "tool_calls": run.tool_calls,
                    "quiet": run.quiet,
                    "resumed": run.resumed,
                });
                assert_eq!(&actual, &case["run"], "{name}");
            } else {
                let problem = read.expect_err(&name.to_string());
                assert_eq!(problem.property, case["problem"]["property"], "{name}");
                assert_eq!(problem.code, case["problem"]["code"], "{name}");
            }
        }
    }

    #[test]
    fn quiet_answers_and_paths_follow_the_fixture() {
        let fixture = fixture();
        for case in cases(&fixture, "quiet") {
            assert_eq!(
                is_quiet(case["answer"].as_str().unwrap()),
                case["quiet"] == true,
                "{}",
                case["answer"]
            );
        }
        for case in cases(&fixture, "paths") {
            if let Some(title) = case.get("title") {
                assert_eq!(orb_folder(title.as_str().unwrap()), case["folder"]);
                continue;
            }
            let orb_path = case["orb_path"].as_str().unwrap();
            assert_eq!(
                run_path(
                    orb_path,
                    case["date"].as_str().unwrap(),
                    case["id"].as_str().unwrap()
                ),
                case["run"]
            );
            assert_eq!(beside(orb_path, MEMORY_FILE), case["memory"]);
            assert_eq!(beside(orb_path, HEARTBEAT_FILE), case["heartbeat"]);
        }
    }

    #[test]
    fn a_new_orb_serializes_in_canonical_order() {
        for case in cases(&fixture(), "serialization") {
            let object = &case["object"];
            let orb = WorkspaceObject {
                id: object["id"].as_str().unwrap().to_owned(),
                object_type: object["type"].as_str().unwrap().to_owned(),
                title: String::new(),
                body: object["body"].as_str().unwrap().to_owned(),
                relative_path: "Orbs/Meetings/Meetings.md".to_owned(),
                revision: String::new(),
                created: object["created"].as_str().map(str::to_owned),
                updated: object["updated"].as_str().map(str::to_owned),
                properties: serde_json::from_value(object["properties"].clone()).unwrap(),
            };
            let bytes = orb_registry().serialize(&orb).unwrap();
            assert_eq!(
                String::from_utf8(bytes).unwrap(),
                case["markdown"],
                "{}",
                case["name"]
            );
            assert!(
                read_orb(&managed(&serde_json::json!({
                    "name": case["name"],
                    "path": "Orbs/Meetings/Meetings.md",
                    "markdown": case["markdown"],
                })))
                .is_ok()
            );
        }
    }
}
