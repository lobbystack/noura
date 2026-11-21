//! Orbs in a running app: Orb files as you or another app write them, a
//! provider that answers by what each request holds, and readers for the
//! run records and the thread.

use std::{
    future::Future,
    path::Path,
    pin::Pin,
    sync::{Arc, Mutex},
};

use app_core::{AiOrb, AiOrbRun, App};
use local_core::{
    AiContentPart, AiProviderEventStream, AiProviderStreamEvent, AiStopReason, AiStreamer,
    AiToolCall, ResolvedAiStreamRequest,
};

use super::ai::{Request, message_line};

/// The Meetings Orb's stable ID.
pub const MEETINGS: &str = "orb_01jd8x3k9q2m7v4c6b1n0p5r8t";

/// The Meetings Orb's file.
pub const MEETINGS_FILE: &str = "Orbs/Meetings/Meetings.md";

/// The Meetings Orb's memory.
pub const MEMORY_FILE: &str = "Orbs/Meetings/Memory.md";

/// An Orb's file with these frontmatter lines, last changed at `updated`,
/// and `body` as its instructions.
pub fn orb_file(id: &str, lines: &str, body: &str, updated: &str) -> Vec<u8> {
    format!(
        "---\nid: {id}\ntype: orb\n{lines}created: 2026-11-01T10:00:00Z\nupdated: {updated}\n---\n{body}"
    )
    .into_bytes()
}

/// The Meetings Orb, listing `tools` and `triggers`, with a memory file.
pub fn meetings(tools: &[&str], triggers: &[&str], updated: &str) -> Vec<(String, Vec<u8>)> {
    let list = |values: &[&str]| {
        if values.is_empty() {
            " []\n".to_owned()
        } else {
            values
                .iter()
                .map(|value| format!("\n- {value}"))
                .collect::<String>()
                + "\n"
        }
    };
    let lines = format!(
        "tools:{}triggers:{}memory: '[[Memory]]'\n",
        list(tools),
        list(triggers)
    );
    vec![
        (
            MEETINGS_FILE.to_owned(),
            orb_file(
                MEETINGS,
                &lines,
                "You turn meetings into follow-ups. Create a task for each commitment.\n",
                updated,
            ),
        ),
        (
            MEMORY_FILE.to_owned(),
            b"---\nid: note_01jd8x3k9q2m7v4c6b1n0p5r8m\ntype: note\n---\n- Ada owns the deck.\n"
                .to_vec(),
        ),
    ]
}

/// What a provider answers: text, or a call to one tool.
#[derive(Debug, Clone)]
pub enum Reply {
    Text(String),
    Call(String, serde_json::Value),
}

type Answer = dyn Fn(&Request) -> Reply + Send + Sync;

/// A provider that answers each request at once, as `answer` says for
/// what the request holds, and records every request.
pub struct Scripted {
    requests: Mutex<Vec<Request>>,
    answer: Box<Answer>,
}

impl Scripted {
    pub fn new(answer: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            requests: Mutex::new(Vec::new()),
            answer: Box::new(answer),
        })
    }

    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }

    /// The requests an Orb's runs sent.
    pub fn orb_requests(&self) -> Vec<Request> {
        self.requests().into_iter().filter(is_orb).collect()
    }
}

/// Whether `request` is an Orb's: Noura's Orb instructions come first.
pub fn is_orb(request: &Request) -> bool {
    request
        .system
        .first()
        .is_some_and(|text| text.contains(", an Orb: a personal agent"))
}

/// Whether `request` carries a tool result.
pub fn has_result(request: &Request) -> bool {
    request
        .messages
        .iter()
        .any(|message| message.starts_with("tool: result"))
}

impl AiStreamer for Scripted {
    fn stream(
        &self,
        request: ResolvedAiStreamRequest,
    ) -> Pin<Box<dyn Future<Output = local_core::Result<AiProviderEventStream>> + Send + '_>> {
        let recorded = Request {
            provider: request.provider.id.clone(),
            kind: request.provider.kind.clone(),
            model: request.provider.model.clone(),
            endpoint: request.provider.endpoint.clone(),
            key: request.credential.clone(),
            system: request
                .messages
                .iter()
                .filter(|message| message.role == "system")
                .map(|message| {
                    message
                        .content
                        .iter()
                        .map(|part| match part {
                            AiContentPart::Text { text } => text.clone(),
                            other => format!("{other:?}"),
                        })
                        .collect()
                })
                .collect(),
            messages: request
                .messages
                .iter()
                .filter(|message| message.role != "system")
                .map(message_line)
                .collect(),
            tools: request.tools.iter().map(|tool| tool.name.clone()).collect(),
        };
        let reply = (self.answer)(&recorded);
        self.requests.lock().unwrap().push(recorded);
        let (text, calls) = match reply {
            Reply::Text(text) => (text, Vec::new()),
            Reply::Call(name, arguments) => (
                String::new(),
                vec![AiToolCall {
                    call_id: format!("call_{}", self.requests.lock().unwrap().len()),
                    name,
                    arguments,
                }],
            ),
        };
        let mut events = Vec::new();
        if !text.is_empty() {
            events.push(Ok(AiProviderStreamEvent::TextDelta(text)));
        }
        events.push(Ok(AiProviderStreamEvent::Completed {
            usage: None,
            stop_reason: Some(AiStopReason::Completed { raw: "stop".into() }),
            tool_calls: calls,
        }));
        Box::pin(async move {
            let stream: AiProviderEventStream = Box::pin(futures::stream::iter(events));
            Ok(stream)
        })
    }
}

/// The Orb `id` as the service lists it.
pub fn orb(app: &App, id: &str) -> AiOrb {
    app.orbs()
        .unwrap()
        .into_iter()
        .find(|orb| orb.id == id)
        .unwrap_or_else(|| panic!("no Orb {id}"))
}

/// The Orb `id`'s runs, the newest first.
pub fn runs(app: &App, id: &str) -> Vec<AiOrbRun> {
    app.orb_runs(id, 50).unwrap()
}

/// Wait until the Orb `id` has `count` runs that ended and works no more.
pub fn wait_for_runs(app: &mut App, id: &str, count: usize) -> Vec<AiOrbRun> {
    let id = id.to_owned();
    super::plugins::wait_until(app, &format!("{count} runs of the Orb end"), |app| {
        let ended = app.orb_runs(&id, 50).is_ok_and(|runs| {
            runs.iter()
                .filter(|run| run.status != app_core::AiOrbRunStatus::Running)
                .count()
                >= count
        });
        ended
            && app.orbs().is_ok_and(|orbs| {
                orbs.iter()
                    .all(|orb| orb.id != id || orb.state != app_core::AiOrbState::Working)
            })
    });
    runs(app, &id)
}

/// Every run record's file under `Orbs/`, by path, with its text.
pub fn run_files(root: &Path) -> Vec<(String, String)> {
    let mut found = Vec::new();
    collect(&root.join("Orbs"), root, &mut found);
    found.retain(|(path, _)| path.contains("/Runs/"));
    found.sort();
    found
}

fn collect(folder: &Path, root: &Path, found: &mut Vec<(String, String)>) {
    for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, root, found);
        } else {
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            found.push((relative, std::fs::read_to_string(&path).unwrap_or_default()));
        }
    }
}
