//! Transcriptions as files: making one with its Transcript, finding the
//! Transcript of one, its project and its folder, and moving it into a
//! project's folder.

use jiff::{Timestamp, Zoned, tz::TimeZone};
use plugin_sdk::{
    Error, clock,
    objects::{self, Changes, NewObject, Object},
    workspace,
};
use serde_json::{Map, Value, json};

use crate::{
    settings::{FolderRule, Settings},
    types::{TRANSCRIPT, TRANSCRIPTION},
};

/// The folder new Transcriptions go in without a project.
pub(crate) const MEETINGS: &str = "Meetings";

/// The Transcript's file name in a Transcription's folder.
pub(crate) const TRANSCRIPT_FILE: &str = "Transcript.md";

/// The device's clock now, in its time zone.
pub(crate) fn now() -> Zoned {
    let reading = clock::now();
    let instant: Timestamp = reading.instant.parse().unwrap_or(Timestamp::UNIX_EPOCH);
    instant.to_zoned(zone(&reading.time_zone))
}

fn zone(name: &str) -> TimeZone {
    if let Some(sign) = name.chars().next().filter(|sign| matches!(sign, '+' | '-')) {
        let (hours, minutes) = name[1..].split_once(':').unwrap_or((&name[1..], "0"));
        let seconds =
            hours.parse::<i32>().unwrap_or(0) * 3600 + minutes.parse::<i32>().unwrap_or(0) * 60;
        let seconds = if sign == '-' { -seconds } else { seconds };
        return jiff::tz::Offset::from_seconds(seconds).map_or(TimeZone::UTC, TimeZone::fixed);
    }
    TimeZone::get(name).unwrap_or(TimeZone::UTC)
}

/// `at` as the workspace format writes a timed date: RFC 3339 with the
/// offset, to the second.
pub(crate) fn rfc3339(at: &Zoned) -> String {
    at.strftime("%Y-%m-%dT%H:%M:%S%:z").to_string()
}

/// `start`, an RFC 3339 instant, plus `ms`, in the device's zone.
pub(crate) fn later(start: &str, ms: u64) -> Option<String> {
    let start: Timestamp = start.parse().ok()?;
    let span = jiff::SignedDuration::from_millis(i64::try_from(ms).ok()?);
    let end = start.checked_add(span).ok()?;
    Some(rfc3339(&end.to_zoned(now().time_zone().clone())))
}

/// The folder that holds the file at `path`, or empty at the top.
pub(crate) fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(folder, _)| folder)
}

/// The folder a project's files go in: its own, as
/// `Projects/Garden/` for `Projects/Garden/Garden.md`, or the one its
/// file sits in.
pub(crate) fn project_folder(project: &str) -> Option<String> {
    let object = workspace::read_object(project).ok()?;
    Some(parent(&object.path).to_owned())
}

/// Every project, as (stable ID, title), by title.
pub(crate) fn projects() -> Vec<(String, String)> {
    let mut projects: Vec<(String, String)> = workspace::list_entries()
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .filter(|entry| entry.object_type.as_deref() == Some("project"))
        .filter_map(|entry| {
            let id = entry.object_id?;
            let name = entry.path.rsplit('/').next().unwrap_or(&entry.path);
            Some((id, name.strip_suffix(".md").unwrap_or(name).to_owned()))
        })
        .collect();
    projects.sort_by_key(|project| project.1.to_lowercase());
    projects.dedup_by(|a, b| a.0 == b.0);
    projects
}

/// The object a property's link leads to from the file at `from`, by ID.
pub(crate) fn resolve(link: &str, from: &str) -> Option<String> {
    workspace::resolve_link(link, Some(from))
        .ok()
        .flatten()
        .map(|object| object.id)
}

/// An object's properties, as a JSON object.
pub(crate) fn properties(object: &Object) -> Map<String, Value> {
    serde_json::from_str(&object.properties).unwrap_or_default()
}

/// The stable ID of the project a Transcription's `project` leads to.
pub(crate) fn project_of(object: &Object) -> Option<String> {
    let link = properties(object).get("project")?.as_str()?.to_owned();
    resolve(&link, &object.path)
}

/// Every path in the workspace, files and folders.
fn taken() -> Vec<String> {
    workspace::list_entries()
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .map(|entry| entry.path)
        .collect()
}

/// A folder for a new Transcription titled `title` under `base`, with a
/// number when the name is taken: `Meetings/Standup 1`.
fn free_folder(base: &str, title: &str) -> (String, String) {
    let taken = taken();
    let lower: Vec<String> = taken.iter().map(|path| path.to_lowercase()).collect();
    for number in 0.. {
        let named = if number == 0 {
            title.to_owned()
        } else {
            format!("{title} {number}")
        };
        let folder = if base.is_empty() {
            named.clone()
        } else {
            format!("{base}/{named}")
        };
        let folder_lower = folder.to_lowercase();
        let used = lower
            .iter()
            .any(|path| *path == folder_lower || path.starts_with(&format!("{folder_lower}/")));
        if !used {
            return (folder, named);
        }
    }
    unreachable!("some number is free")
}

/// The title a new Transcription gets, from today's date.
pub(crate) fn new_title() -> String {
    format!("Transcription {}", now().date())
}

/// Make a Transcription titled for today, with its empty Transcript, in
/// `Meetings/` or the folder of `project` when the setting says so.
pub(crate) fn create(project: Option<&str>) -> Result<Object, Error> {
    let settings = Settings::read();
    let base = match (settings.folder, project) {
        (FolderRule::Project, Some(project)) => {
            project_folder(project).unwrap_or_else(|| MEETINGS.to_owned())
        }
        _ => MEETINGS.to_owned(),
    };
    let (folder, title) = free_folder(&base, &new_title());
    let mut set = Map::new();
    if let Some(project) = project {
        set.insert("project".into(), json!(workspace::link_to(project)?));
    }
    let object = objects::create(&NewObject {
        object_type: TRANSCRIPTION.to_owned(),
        title: title.clone(),
        path: Some(format!("{folder}/{title}.md")),
        body: String::new(),
        properties: Value::Object(set).to_string(),
    })?;
    objects::create(&NewObject {
        object_type: TRANSCRIPT.to_owned(),
        title: String::new(),
        path: Some(format!("{folder}/{TRANSCRIPT_FILE}")),
        body: String::new(),
        properties: json!({ "transcription": workspace::link_to(&object.id)? }).to_string(),
    })?;
    Ok(object)
}

/// The Transcript of the Transcription `id`: the `transcript` whose link
/// leads to it, preferring the one in its folder.
pub(crate) fn transcript_of(transcription: &Object) -> Option<Object> {
    let folder = parent(&transcription.path);
    let mut found: Vec<Object> = objects::list(TRANSCRIPT)
        .ok()?
        .into_iter()
        .filter(|transcript| {
            properties(transcript)
                .get("transcription")
                .and_then(Value::as_str)
                .and_then(|link| resolve(link, &transcript.path))
                .is_some_and(|id| id == transcription.id)
        })
        .collect();
    found.sort_by_key(|transcript| parent(&transcript.path) != folder);
    found.into_iter().next()
}

/// The Transcript of `transcription`, made in its folder when it has
/// none, as after you deleted it.
pub(crate) fn transcript_or_new(transcription: &Object) -> Result<Object, Error> {
    if let Some(transcript) = transcript_of(transcription) {
        return Ok(transcript);
    }
    let folder = parent(&transcription.path);
    let path = if folder.is_empty() {
        TRANSCRIPT_FILE.to_owned()
    } else {
        format!("{folder}/{TRANSCRIPT_FILE}")
    };
    objects::create(&NewObject {
        object_type: TRANSCRIPT.to_owned(),
        title: String::new(),
        path: Some(path),
        body: String::new(),
        properties: json!({ "transcription": workspace::link_to(&transcription.id)? }).to_string(),
    })
}

/// Change the object `id`, reading it again when it changed meanwhile,
/// as when you typed in it: `change` gets the file as it is now.
pub(crate) fn update(id: &str, change: impl Fn(&Object) -> Changes) -> Result<Object, Error> {
    let mut last = None;
    for _ in 0..3 {
        let object = objects::get(id)?;
        match objects::update(id, &object.revision, &change(&object)) {
            Err(Error::RevisionConflict(conflict)) => {
                last = Some(Error::RevisionConflict(conflict))
            }
            done => return done,
        }
    }
    Err(last.unwrap_or_else(|| Error::RevisionConflict(id.to_owned())))
}

/// Set properties of the object `id` and keep its body.
pub(crate) fn set(id: &str, properties: Value) -> Result<Object, Error> {
    update(id, |_| Changes {
        body: None,
        properties: properties.to_string(),
        remove: Vec::new(),
    })
}

/// Give the Transcription `id` the project `project`, or none, and move
/// its folder into that project's folder when the setting puts
/// Transcriptions there.
pub(crate) fn set_project(id: &str, project: Option<&str>) -> Result<(), Error> {
    let object = match project {
        Some(project) => set(id, json!({ "project": workspace::link_to(project)? }))?,
        None => update(id, |_| Changes {
            body: None,
            properties: "{}".to_owned(),
            remove: vec!["project".to_owned()],
        })?,
    };
    let (FolderRule::Project, Some(project)) = (Settings::read().folder, project) else {
        return Ok(());
    };
    let Some(base) = project_folder(project) else {
        return Ok(());
    };
    let folder = parent(&object.path);
    if parent(folder) == base {
        return Ok(());
    }
    // A Transcription in a folder of its own moves with that folder's
    // name; one elsewhere gets a folder named after its title.
    let name = folder.rsplit('/').next().unwrap_or(folder);
    let own_folder = !folder.is_empty() && name == object.title;
    let (destination, _) = free_folder(&base, if own_folder { name } else { &object.title });
    // The whole folder moves, the saved audio with it, and leaves no
    // empty folder behind. A folder that also holds other notes keeps
    // them: then only the Transcription and its Transcript move.
    if own_folder && objects::move_folder(&object.id, &object.revision, &destination).is_ok() {
        return Ok(());
    }
    let transcript = transcript_of(&object);
    objects::move_to(
        &object.id,
        &object.revision,
        &format!("{destination}/{}.md", object.title),
    )?;
    if let Some(transcript) = transcript {
        let file = transcript
            .path
            .rsplit('/')
            .next()
            .unwrap_or(TRANSCRIPT_FILE);
        objects::move_to(
            &transcript.id,
            &transcript.revision,
            &format!("{destination}/{file}"),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parents_and_times() {
        assert_eq!(parent("Meetings/Standup/Standup.md"), "Meetings/Standup");
        assert_eq!(parent("Standup.md"), "");
        let start = "2026-11-16T09:00:00-05:00";
        let ts: Timestamp = start.parse().unwrap();
        let zoned = ts.to_zoned(zone("-05:00"));
        assert_eq!(rfc3339(&zoned), start);
        assert_eq!(zone("America/Toronto").iana_name(), Some("America/Toronto"));
    }
}
