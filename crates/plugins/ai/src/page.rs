//! The Orbs page: the open Orb's thread as a conversation under the pill
//! with its name, which opens its side panel. Your messages and its
//! answers, the answer streaming in, the tool calls and web searches as
//! quiet rows, Resume, and the box for the next message with Stop.

use std::collections::BTreeMap;

use plugin_sdk::{
    ActivityState, Event, MessageAuthor, Node, View,
    ai::{
        self, Chat, Message, MessageKind, MessageStatus, Orb, OrbState, Request, Run, RunStatus,
        ToolDefinition,
    },
    navigation::{self, SettingsSection},
};

use crate::{State, failures, look, panel, reason, web};

/// The node IDs events name.
pub(crate) const PILL: &str = "orb-pill";
/// The empty state's button that opens Settings on AI.
const OPEN_AI_SETTINGS: &str = "open-ai-settings";
const CONVERSATION: &str = "conversation";
/// The message box. A press of its stop button names it too.
const COMPOSER: &str = "composer";
const RESUME: &str = "resume";

/// The Orb the page shows: the one you picked while it's still there, or
/// the default Orb, which `orbs` lists first.
pub(crate) fn open_orb(state: &mut State, orbs: &[Orb]) -> Option<Orb> {
    let picked = state
        .orb
        .as_ref()
        .and_then(|id| orbs.iter().find(|orb| &orb.id == id))
        .cloned();
    if picked.is_none() {
        state.orb = None;
    }
    picked.or_else(|| orbs.first().cloned())
}

/// Show the chat `chat`: the default Orb's plain chat, or an Orb's thread.
pub(crate) fn open_chat(state: &mut State, chat: &str) {
    let orb = ai::read_chat(chat).ok().and_then(|(chat, _)| chat.orb);
    state.orb = orb.filter(|orb| orb != look::DEFAULT_ORB);
    state.chat = Some(chat.to_owned());
    state.new_chat = false;
}

/// The default Orb's latest plain chat, which the page shows until you
/// pick another or start one.
fn latest_plain_chat() -> Option<String> {
    ai::chats()
        .ok()?
        .into_iter()
        .find(|chat| chat.orb.is_none())
        .map(|chat| chat.id)
}

/// The chat the page shows for `orb`, and its messages, or why it can't.
fn read_thread(state: &mut State, orb: &Orb) -> (Option<Chat>, Vec<Message>, Option<String>) {
    if orb.id != look::DEFAULT_ORB {
        return match ai::read_orb(&orb.id) {
            Ok((_, chat, messages)) => (chat, messages, None),
            Err(error) => (None, Vec::new(), Some(reason(&error))),
        };
    }
    if !state.new_chat && state.chat.is_none() {
        state.chat = latest_plain_chat();
    }
    let Some(id) = state.chat.clone().filter(|_| !state.new_chat) else {
        return (None, Vec::new(), None);
    };
    match ai::read_chat(&id) {
        Ok((chat, messages)) => (Some(chat), messages, None),
        Err(error) => {
            // A chat that went, as when its folder moved to the trash,
            // leaves a new chat in its place.
            if matches!(error, plugin_sdk::Error::NotFound(_)) {
                state.chat = None;
                state.new_chat = true;
            }
            (None, Vec::new(), Some(reason(&error)))
        }
    }
}

/// Where the page keeps the run of the last message you sent to `orb`:
/// under the Orb's ID, or for the default Orb under the chat's.
fn run_key(state: &State, orb: &Orb) -> Option<String> {
    if orb.id == look::DEFAULT_ORB {
        state.chat.clone().filter(|_| !state.new_chat)
    } else {
        Some(orb.id.clone())
    }
}

/// The run that works in `orb`'s thread, or last did: the last message
/// you sent it, or while the Orb works, the run a trigger started.
fn current_run(state: &State, orb: &Orb) -> Option<Run> {
    let sent = run_key(state, orb)
        .and_then(|key| state.runs.get(&key))
        .and_then(|id| ai::read_run(id).ok());
    if orb.id == look::DEFAULT_ORB
        || orb.state != OrbState::Working
        || sent.as_ref().is_some_and(busy)
    {
        return sent;
    }
    ai::orb_runs(&orb.id, 1)
        .ok()
        .and_then(|runs| runs.into_iter().next())
        .filter(|run| run.status == ai::OrbRunStatus::Running)
        .and_then(|run| ai::read_run(&run.id).ok())
        .or(sent)
}

/// The page: the pill with the Orb's name, its thread as a conversation
/// with the message box at the bottom, or a way to connect a provider,
/// and its side panel while that's open.
pub(crate) fn render(state: &mut State) -> Vec<Node> {
    hand_over_tool_calls(state);
    let mut view = View::panel("ai", Some("Orbs"));
    let providers = match ai::providers() {
        Ok(providers) => providers,
        Err(error) => {
            view.text(View::ROOT, "providers-problem", &reason(&error));
            return view.into_nodes();
        }
    };
    // Without the Orbs, as while the index can't be read, the page still
    // shows the default Orb's chats.
    let (orbs, orbs_problem) = match ai::orbs() {
        Ok(orbs) => (orbs, None),
        Err(error) => (vec![look::default_orb()], Some(reason(&error))),
    };
    let Some(orb) = open_orb(state, &orbs) else {
        return view.into_nodes();
    };
    let (chat, messages, problem) = read_thread(state, &orb);
    let run = current_run(state, &orb);
    let busy = run.as_ref().is_some_and(busy)
        || (orb.id != look::DEFAULT_ORB && orb.state == OrbState::Working);
    view.pill(
        View::ROOT,
        PILL,
        &orb.title,
        Some(look::avatar(&orb)),
        state.panel,
    );
    let talk = view.conversation(View::ROOT, CONVERSATION, &orb.title);
    if chat
        .as_ref()
        .is_some_and(|chat| chat.retention == ai::Retention::Ephemeral)
    {
        view.text(
            talk,
            "retention",
            "This chat moves to the trash 30 days after it started.",
        );
    }
    if let Some(problem) = &orbs_problem {
        view.text(talk, "orbs-problem", problem);
    }
    if let Some(problem) = problem
        .as_ref()
        .filter(|problem| Some(*problem) != orbs_problem.as_ref())
    {
        view.text(talk, "chat-problem", problem);
    }
    draw_messages(
        &mut view,
        talk,
        &messages,
        run.as_ref(),
        &orb.title,
        &providers,
    );
    if let Some(run) = &run {
        // A message Noura didn't write to the chat, as when you said no or
        // the write failed, comes back to the message box.
        if run.message_written {
            state.unsent.remove(&run.id);
        } else if !busy && let Some(message) = state.unsent.remove(&run.id) {
            state.draft = message;
        }
        draw_run_status(&mut view, talk, run, &providers);
    }
    if let Some(notice) = &state.notice {
        view.text(talk, "notice", notice);
    }
    if providers.is_empty() && !busy {
        let empty = view.empty_state(talk, "no-providers", "Connect an AI provider", None, None);
        view.button(empty, OPEN_AI_SETTINGS, "Open AI settings");
    } else {
        if messages.is_empty() && problem.is_none() && state.notice.is_none() {
            view.empty_state(talk, "start", "How can I help?", None, None);
        }
        // Noura takes up an Orb's runs by itself after a quit.
        if !busy && orb.id == look::DEFAULT_ORB && stopped_before_answering(&messages) {
            view.button(talk, RESUME, "Resume");
        }
        // OpenAI asks apps to say when a request uses your ChatGPT plan,
        // with a way to manage it.
        if look::provider_of(&orb, &providers).is_some_and(|provider| provider.kind == "chatgpt") {
            view.markdown(
                talk,
                "chatgpt-plan",
                &format!(
                    "Using ChatGPT plan · [Manage usage]({})",
                    failures::MANAGE_USAGE
                ),
            );
        }
        let composer = view.composer(
            talk,
            COMPOSER,
            &format!("Message {}", orb.title),
            &state.draft,
            "Send",
        );
        if busy {
            view.stop(composer, "Stop");
        }
    }
    if state.panel {
        panel::draw(&mut view, state, &orb, chat.as_ref());
    }
    view.into_nodes()
}

/// Whether a run is still at work: Noura asks you about a tool call, or
/// the answer streams in.
fn busy(run: &Run) -> bool {
    matches!(
        run.status,
        RunStatus::Streaming | RunStatus::WaitingForApproval
    )
}

/// The tools a plain chat's message offers the model: those of the
/// plugins on in the workspace, then web search and page reads while they
/// work.
fn offered_tools() -> Vec<ToolDefinition> {
    let mut tools = ai::workspace_tools().unwrap_or_default();
    tools.extend(web::tools());
    tools
}

/// Settle each plain chat run's tool calls once the model asked for them.
/// The plugin answers a turn of web calls itself, through the host AI
/// interface. Noura gets any other turn: it asks you about each call in
/// turn and runs the ones you allow. Then the model answers again in the
/// same run. Noura runs an Orb's calls itself.
fn hand_over_tool_calls(state: &mut State) {
    let mut problem = None;
    let ids: Vec<String> = state
        .runs
        .values()
        .filter(|id| !state.orb_runs.contains(*id))
        .cloned()
        .collect();
    for id in ids {
        let Ok(run) = ai::read_run(&id) else {
            continue;
        };
        if run.status != RunStatus::Completed || run.tool_calls.is_empty() {
            continue;
        }
        let web_only = run
            .tool_calls
            .iter()
            .all(|call| matches!(call.name.as_str(), web::SEARCH | web::FETCH));
        if !web_only {
            if let Err(error) = ai::run_tools(&id) {
                problem = Some(reason(&error));
            }
            continue;
        }
        let turn = std::iter::once(run.id.as_str())
            .chain(run.tool_calls.iter().map(|call| call.id.as_str()))
            .collect::<Vec<_>>()
            .join(" ");
        if state.failed_turns.contains(&turn) {
            continue;
        }
        if let Err(error) = web::answer(&run, &offered_tools()) {
            problem = Some(reason(&error));
            state.failed_turns.insert(turn);
        }
    }
    if problem.is_some() {
        state.notice = problem;
    }
}

/// Whether Noura stopped before your last message got its answer, as
/// after a crash or a quit: a message after it is interrupted, with no
/// code that says a plugin went, and no answer completed since.
fn stopped_before_answering(messages: &[Message]) -> bool {
    let Some(last) = messages
        .iter()
        .rposition(|message| message.kind == MessageKind::User)
    else {
        return false;
    };
    let after = &messages[last + 1..];
    let Some(cut) = after.iter().rposition(|message| {
        message.status == MessageStatus::Interrupted && message.error_code.is_none()
    }) else {
        return false;
    };
    !after[cut + 1..].iter().any(|message| {
        message.kind == MessageKind::Assistant && message.status == MessageStatus::Completed
    })
}

/// The thread's messages in the conversation at `talk`, the answers
/// named `who` for screen readers. Each tool call shows with the result
/// that answers it, which pairs with the latest call before it of the
/// same tool that has none yet.
fn draw_messages(
    view: &mut View,
    talk: u32,
    messages: &[Message],
    run: Option<&Run>,
    who: &str,
    providers: &[ai::Provider],
) {
    let mut answers: BTreeMap<usize, usize> = BTreeMap::new();
    for (index, message) in messages.iter().enumerate() {
        if message.kind != MessageKind::ToolResult {
            continue;
        }
        let call = (0..index).rev().find(|&call| {
            messages[call].kind == MessageKind::ToolCall
                && messages[call].tool_name == message.tool_name
                && !answers.contains_key(&call)
        });
        if let Some(call) = call {
            answers.insert(call, index);
        }
    }
    let answered: Vec<usize> = answers.values().copied().collect();
    for (index, message) in messages.iter().enumerate() {
        match message.kind {
            MessageKind::ToolCall => {
                let result = answers.get(&index).map(|&result| &messages[result]);
                draw_tool_call(view, talk, message, result, run);
            }
            MessageKind::ToolResult if answered.contains(&index) => {}
            _ => draw_message(view, talk, message, run, who, providers),
        }
    }
}

/// One message of the thread. The answer that streams in shows the text
/// that arrived so far.
fn draw_message(
    view: &mut View,
    talk: u32,
    message: &Message,
    run: Option<&Run>,
    who: &str,
    providers: &[ai::Provider],
) {
    match message.kind {
        MessageKind::User => {
            view.message(
                talk,
                &message.id,
                MessageAuthor::You,
                "You",
                &message.content,
            );
        }
        MessageKind::Assistant => {
            let streaming = run.filter(|run| run.message.as_deref() == Some(message.id.as_str()));
            let text = match (message.status, streaming) {
                (MessageStatus::InProgress, Some(run)) => run.text.as_str(),
                _ => message.content.as_str(),
            };
            // A turn that only asked for tools has no text: its calls
            // show it.
            if text.trim().is_empty() && message.status == MessageStatus::Completed {
                return;
            }
            let shown = view.message(talk, &message.id, MessageAuthor::Other, who, text);
            match message.status {
                MessageStatus::InProgress if streaming.is_none() => {
                    view.note(shown, "Not finished.");
                }
                MessageStatus::InProgress | MessageStatus::Completed => {}
                MessageStatus::Cancelled => view.note(shown, "Stopped."),
                MessageStatus::Interrupted
                    if message.error_code.as_deref() == Some("plugin-disabled") =>
                {
                    view.warn(shown, "You turned off a plugin this answer used.");
                }
                MessageStatus::Interrupted => {
                    view.note(shown, "Noura stopped before this answer finished.");
                }
                MessageStatus::Failed => {
                    let provider = failures::provider_name(message.provider.as_deref(), providers);
                    view.warn(
                        shown,
                        &failures::line(message.error_code.as_deref(), provider),
                    );
                    if message.error_code.as_deref() == Some("provider_plan_limit") {
                        view.markdown(
                            talk,
                            &format!("{}-usage", message.id),
                            &format!("[Manage usage]({})", failures::MANAGE_USAGE),
                        );
                    }
                }
            }
        }
        // A result whose call isn't in the chat shows on its own.
        MessageKind::ToolCall | MessageKind::ToolResult => {
            draw_tool_call(view, talk, message, None, run);
        }
        MessageKind::ContextSummary => {
            let summary = view.activity(
                talk,
                &message.id,
                "Summarized the messages above",
                ActivityState::Done,
            );
            view.markdown(summary, &format!("{}-text", message.id), &message.content);
        }
    }
}

/// Whether a tool call or result message is a web search's or a page
/// read's: a plain chat's, or an Orb's through Noura's own tools.
fn is_web_call(message: &Message) -> bool {
    matches!(
        message.tool_name.as_deref(),
        Some(web::SEARCH | web::FETCH | "noura__web_search" | "noura__web_fetch")
    )
}

/// What a tool's name reads as: `create from Tasks` for the Tasks
/// plugin's `create` command, or the name itself for another tool.
fn tool_title(name: &str) -> String {
    match name.split_once("__") {
        Some((plugin, command)) if !plugin.is_empty() && !command.is_empty() => {
            let mut letters = plugin.chars();
            let plugin: String = letters
                .next()
                .map(|first| first.to_uppercase().chain(letters).collect())
                .unwrap_or_default();
            format!("{command} from {plugin}")
        }
        _ => name.to_owned(),
    }
}

/// A tool call as a quiet row in `talk`, with the result that answers it
/// once there is one: what it asked for and what came back show when you
/// open it. `message` may be a result whose call isn't in the chat.
fn draw_tool_call(
    view: &mut View,
    talk: u32,
    message: &Message,
    result: Option<&Message>,
    run: Option<&Run>,
) {
    let (call, result) = match message.kind {
        MessageKind::ToolResult => (None, Some(message)),
        _ => (Some(message), result),
    };
    let web = is_web_call(message);
    let denied = result.is_some_and(|result| {
        serde_json::from_str::<serde_json::Value>(&result.content)
            .is_ok_and(|result| result["denied"] == true)
    });
    let shown_result = result
        .filter(|_| web)
        .and_then(|result| web::shown_result(&result.content));
    let failed_web = matches!(shown_result, Some(web::Shown::Failed(_)));
    let tool = message.tool_name.as_deref().unwrap_or("a tool");
    let title = match call
        .filter(|_| web)
        .and_then(|call| web::shown_call(&call.content))
    {
        Some(web::Shown::Searched(query)) => {
            format!("Searched the web for \u{201c}{query}\u{201d}")
        }
        Some(web::Shown::Read(url)) => format!("Read {url}"),
        _ => tool_title(tool),
    };
    let waiting = run.is_some_and(|run| run.status == RunStatus::WaitingForApproval);
    let status = call.map_or(MessageStatus::Completed, |call| call.status);
    let state = match status {
        MessageStatus::InProgress if waiting => ActivityState::Waiting,
        MessageStatus::InProgress => ActivityState::Running,
        MessageStatus::Completed if denied => ActivityState::Denied,
        MessageStatus::Completed if failed_web => ActivityState::Failed,
        MessageStatus::Completed => ActivityState::Done,
        MessageStatus::Cancelled | MessageStatus::Interrupted => ActivityState::Stopped,
        MessageStatus::Failed => ActivityState::Failed,
    };
    let row = view.activity(talk, &message.id, &title, state);
    let note = match status {
        MessageStatus::Interrupted
            if call.and_then(|call| call.error_code.as_deref()) == Some("plugin-disabled") =>
        {
            Some("You turned off this tool's plugin, so the call didn't run.")
        }
        MessageStatus::Interrupted => {
            Some("Noura stopped before this call finished and won't rerun it.")
        }
        MessageStatus::Failed => Some("This call failed."),
        _ if denied => Some("You denied this call, so it didn't run."),
        _ => None,
    };
    if let Some(note) = note {
        view.text(row, &format!("{}-status", message.id), note);
    }
    if web {
        draw_web_result(view, row, &message.id, shown_result);
        return;
    }
    if let Some(call) = call {
        view.markdown(row, &format!("{}-input", message.id), &code(&call.content));
    }
    if let Some(result) = result.filter(|_| !denied) {
        view.markdown(
            row,
            &format!("{}-result", message.id),
            &code(&result.content),
        );
    }
}

/// JSON text as a Markdown code block, laid out on several lines.
fn code(json: &str) -> String {
    let pretty = serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| json.to_owned());
    format!("```\n{pretty}\n```")
}

/// What a web search or a page read brought back, in its row at `row`:
/// the sources a search found, or why the call failed.
fn draw_web_result(view: &mut View, row: u32, id: &str, shown: Option<web::Shown>) {
    let id = format!("{id}-web");
    match shown {
        Some(web::Shown::Sources(sources)) if sources.is_empty() => {
            view.text(row, &id, "The search found nothing.");
        }
        Some(web::Shown::Sources(sources)) => {
            let list = view.list(row, &id, "Sources");
            for (index, (title, url)) in sources.iter().enumerate() {
                view.item(list, &format!("{id}-{index}"), title, Some(url));
            }
        }
        Some(web::Shown::Failed(reason)) => {
            view.text(row, &id, &format!("The web call failed: {reason}"));
        }
        Some(web::Shown::Searched(_) | web::Shown::Read(_) | web::Shown::Page) | None => {}
    }
}

/// What the run of the last message you sent says, when the messages
/// don't: it ended early.
/// A failed run's answer message carries its provider's failure; a run
/// that failed before it wrote one, or for a reason of Noura's own, says
/// so here.
fn draw_run_status(view: &mut View, talk: u32, run: &Run, providers: &[ai::Provider]) {
    let line = match run.status {
        RunStatus::Failed => run.failure.as_ref().and_then(|failure| {
            if !failures::speaks_for(&failure.code) {
                Some(format!("The answer failed: {}", failure.message))
            } else if run.message.is_none() {
                let provider = failures::provider_name(Some(&run.provider), providers);
                Some(failures::line(Some(&failure.code), provider))
            } else {
                None
            }
        }),
        RunStatus::Interrupted => {
            Some("You turned off a plugin this answer relied on, so it stopped.".to_owned())
        }
        RunStatus::Streaming
        | RunStatus::WaitingForApproval
        | RunStatus::Completed
        | RunStatus::Cancelled => None,
    };
    if let Some(line) = line {
        view.text(talk, "run-status", &line);
    }
}

/// Something you did on the page, in its side panel or in its sidebar
/// section.
pub(crate) fn handle(state: &mut State, event: Event) {
    match event {
        Event::Pressed(node) if node == PILL => {
            state.panel = !state.panel;
            state.panel_notice = None;
        }
        Event::Pressed(node) if node == OPEN_AI_SETTINGS => {
            if let Err(error) = navigation::open_settings(SettingsSection::Ai) {
                state.notice = Some(reason(&error));
            }
        }
        // The message box's stop button.
        Event::Pressed(node) if node == COMPOSER => stop(state),
        Event::Pressed(node) if node == RESUME => resume(state),
        Event::Submitted(submission) if submission.node == COMPOSER => {
            let message = submission
                .fields
                .into_iter()
                .find(|field| field.node == COMPOSER)
                .and_then(|field| match field.value {
                    plugin_sdk::Value::Multiline(text) => Some(text),
                    _ => None,
                })
                .unwrap_or_default();
            send(state, message);
        }
        event => {
            if !crate::sidebar::handle(state, &event) {
                panel::handle(state, event);
            }
        }
    }
}

/// Stop the run of the last message you sent to the open Orb.
fn stop(state: &mut State) {
    let orbs = ai::orbs().unwrap_or_default();
    let Some(orb) = open_orb(state, &orbs) else {
        return;
    };
    if let Some(run) = current_run(state, &orb)
        && let Err(error) = ai::stop(&run.id)
    {
        state.notice = Some(reason(&error));
    }
}

/// Answer your last message in the default Orb's open chat again, with
/// its provider.
fn resume(state: &mut State) {
    let Some(chat) = state.chat.clone().filter(|_| !state.new_chat) else {
        return;
    };
    state.notice = None;
    let providers = ai::providers().unwrap_or_default();
    let orbs = ai::orbs().unwrap_or_default();
    let Some(provider) = orbs
        .first()
        .and_then(|orb| look::provider_of(orb, &providers))
    else {
        return;
    };
    match ai::resume(&chat, &provider.id, &offered_tools()) {
        Ok(run) => {
            state.runs.insert(chat, run.id);
        }
        Err(error) => state.notice = Some(reason(&error)),
    }
}

/// Send `message` to the open Orb: to its thread, or for the default Orb
/// to its open chat or a new one, from its provider, offering the tools of
/// the plugins on in the workspace. A message that doesn't go out comes
/// back to the message box.
fn send(state: &mut State, message: String) {
    if message.trim().is_empty() {
        return;
    }
    state.notice = None;
    let orbs = ai::orbs().unwrap_or_default();
    let Some(orb) = open_orb(state, &orbs) else {
        state.draft = message;
        return;
    };
    if orb.id != look::DEFAULT_ORB {
        match ai::send_to_orb(&orb.id, &message) {
            Ok(run) => {
                if !run.message_written {
                    state.unsent.insert(run.id.clone(), message);
                }
                state.orb_runs.insert(run.id.clone());
                state.runs.insert(orb.id.clone(), run.id);
                state.draft.clear();
            }
            Err(error) => {
                state.notice = Some(reason(&error));
                state.draft = message;
            }
        }
        return;
    }
    let providers = ai::providers().unwrap_or_default();
    let Some(provider) = look::provider_of(&orb, &providers) else {
        state.draft = message;
        return;
    };
    let chat = match state.chat.clone().filter(|_| !state.new_chat) {
        Some(chat) => chat,
        None => match ai::create_chat() {
            Ok(chat) => chat.id,
            Err(error) => {
                state.notice = Some(reason(&error));
                state.draft = message;
                return;
            }
        },
    };
    state.chat = Some(chat.clone());
    state.new_chat = false;
    let request = Request {
        chat: chat.clone(),
        provider: provider.id,
        message: message.clone(),
        tools: offered_tools(),
    };
    match ai::send(&request) {
        Ok(run) => {
            if !run.message_written {
                state.unsent.insert(run.id.clone(), message);
            }
            state.runs.insert(chat, run.id);
            state.draft.clear();
        }
        Err(error) => {
            state.notice = Some(reason(&error));
            state.draft = message;
        }
    }
}
