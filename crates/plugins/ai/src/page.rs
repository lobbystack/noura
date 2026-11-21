//! The AI page and its sidebar section: the chats, each with a menu to
//! rename it or have it removed after 30 days, and the open chat as a
//! conversation: your messages and the answers, the answer streaming in,
//! the tool calls and web searches as quiet rows, Resume, and the box for
//! the next message with its provider and Stop.

use std::collections::BTreeMap;

use plugin_sdk::{
    ActivityState, Event, MessageAuthor, Node, Value, View,
    ai::{
        self, Chat, Message, MessageKind, MessageStatus, Provider, Request, Retention, Run,
        RunStatus, ToolDefinition,
    },
    navigation::{self, SettingsSection},
};

use crate::{State, reason, web};

/// The node IDs events name.
const NEW_CHAT_BUTTON: &str = "new-chat";
/// The empty state's button that opens Settings on AI.
const OPEN_AI_SETTINGS: &str = "open-ai-settings";
const CHATS: &str = "chats";
const CONVERSATION: &str = "conversation";
/// The message box. A press of its stop button names it too.
const COMPOSER: &str = "composer";
const PROVIDER: &str = "provider";
const RESUME: &str = "resume";
const CHAT_NAME: &str = "chat-name";

/// The menu items of a chat in the sidebar, each followed by the chat's
/// ID, a colon and the revision of its `chat.md` the menu showed, which
/// the change carries: a chat that changed elsewhere since keeps that
/// change.
const RENAME: &str = "rename:";
const KEEP: &str = "keep:";
const EPHEMERAL: &str = "ephemeral:";

/// The page: the chat's title, then the chat as a conversation, with the
/// message box at the bottom, or a way to connect a provider.
pub(crate) fn render(state: &mut State) -> Vec<Node> {
    hand_over_tool_calls(state);
    let mut view = View::panel("ai", Some("AI"));
    let providers = match ai::providers() {
        Ok(providers) => providers,
        Err(error) => {
            view.page_heading(View::ROOT, "title", "AI", None, None);
            view.text(View::ROOT, "providers-problem", &reason(&error));
            return view.into_nodes();
        }
    };
    let mut chat = None;
    let mut messages = Vec::new();
    let mut problem = None;
    match state.chat.clone().map(|id| ai::read_chat(&id)) {
        None => {}
        Some(Ok((read, read_messages))) => {
            chat = Some(read);
            messages = read_messages;
        }
        Some(Err(error)) => {
            // A chat that went, as when its folder moved to the trash,
            // leaves a new chat in its place.
            if matches!(error, plugin_sdk::Error::NotFound(_)) {
                state.chat = None;
            }
            problem = Some(reason(&error));
        }
    }
    let run = state
        .chat
        .as_ref()
        .and_then(|chat| state.runs.get(chat))
        .and_then(|id| ai::read_run(id).ok());
    let busy = run.as_ref().is_some_and(busy);
    draw_title(&mut view, chat.as_ref());
    let talk = view.conversation(
        View::ROOT,
        CONVERSATION,
        chat.as_ref().map_or("New chat", |chat| chat.title.as_str()),
    );
    if let Some(problem) = &problem {
        view.text(talk, "chat-problem", problem);
    }
    draw_messages(&mut view, talk, &messages, run.as_ref(), &providers);
    if let Some(run) = &run {
        // A message Noura didn't write to the chat, as when you said no or
        // the write failed, comes back to the message box.
        if run.message_written {
            state.unsent.remove(&run.id);
        } else if !busy && let Some(message) = state.unsent.remove(&run.id) {
            state.draft = message;
        }
        draw_run_status(&mut view, talk, run);
    }
    if let Some(notice) = &state.notice {
        view.text(talk, "notice", notice);
    }
    if providers.is_empty() && !busy {
        let empty = view.empty_state(talk, "no-providers", "Connect an AI provider", None, None);
        view.button(empty, OPEN_AI_SETTINGS, "Open AI settings");
        return view.into_nodes();
    }
    if messages.is_empty() && problem.is_none() && state.notice.is_none() {
        view.empty_state(talk, "start", "How can I help?", None, None);
    }
    if !busy && stopped_before_answering(&messages) {
        view.button(talk, RESUME, "Resume");
    }
    draw_composer(&mut view, talk, state, &providers, &messages, busy);
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

/// The tools a message offers the model: those of the plugins on in the
/// workspace, then web search and page reads while they work.
fn offered_tools() -> Vec<ToolDefinition> {
    let mut tools = ai::workspace_tools().unwrap_or_default();
    tools.extend(web::tools());
    tools
}

/// Settle each run's tool calls once the model asked for them. The plugin
/// answers a turn of web calls itself, through the host AI interface.
/// Noura gets any other turn: it asks you about each call in turn and
/// runs the ones you allow. Then the model answers again in the same run.
fn hand_over_tool_calls(state: &mut State) {
    let mut problem = None;
    let ids: Vec<String> = state.runs.values().cloned().collect();
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

/// The chat's title, and how long Noura keeps it when that isn't until
/// you delete it, or New chat before the first message.
fn draw_title(view: &mut View, chat: Option<&Chat>) {
    let Some(chat) = chat else {
        view.page_heading(View::ROOT, "title", "New chat", None, None);
        return;
    };
    let detail = (chat.retention == Retention::Ephemeral)
        .then_some("This chat moves to the trash 30 days after it started.");
    view.page_heading(View::ROOT, "title", &chat.title, detail, None);
}

/// The menu of a chat in the sidebar: rename it, or choose how long Noura
/// keeps it.
fn draw_chat_menu(view: &mut View, item: u32, chat: &Chat) {
    let menu = view.menu(item, &format!("menu:{}", chat.id), "Chat options");
    let item = |action: &str| format!("{action}{}:{}", chat.id, chat.revision);
    view.menu_item(menu, &item(RENAME), "Rename", true);
    view.separator(menu, &format!("separator:{}", chat.id));
    view.menu_item(
        menu,
        &item(KEEP),
        "Keep until I delete it",
        chat.retention != Retention::Permanent,
    );
    view.menu_item(
        menu,
        &item(EPHEMERAL),
        "Remove after 30 days",
        chat.retention != Retention::Ephemeral,
    );
}

/// The chat's messages in the conversation at `talk`. Each tool call
/// shows with the result that answers it, which pairs with the latest
/// call before it of the same tool that has none yet.
fn draw_messages(
    view: &mut View,
    talk: u32,
    messages: &[Message],
    run: Option<&Run>,
    providers: &[Provider],
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
            _ => draw_message(view, talk, message, run, providers),
        }
    }
}

/// One message of the chat. The answer that streams in shows the text
/// that arrived so far.
fn draw_message(
    view: &mut View,
    talk: u32,
    message: &Message,
    run: Option<&Run>,
    providers: &[Provider],
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
            let provider = message.provider.as_deref().unwrap_or_default();
            let name = providers
                .iter()
                .find(|known| known.id == provider)
                .map_or(provider, |known| known.name.as_str());
            let model = message.model.as_deref().unwrap_or_default();
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
            let shown = view.message(
                talk,
                &message.id,
                MessageAuthor::Other,
                &format!("{name}, {model}"),
                text,
            );
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
                MessageStatus::Failed => view.warn(shown, "This answer failed."),
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
/// read's.
fn is_web_call(message: &Message) -> bool {
    matches!(message.tool_name.as_deref(), Some(web::SEARCH | web::FETCH))
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
fn draw_run_status(view: &mut View, talk: u32, run: &Run) {
    let line = match run.status {
        RunStatus::Failed => run
            .failure
            .as_ref()
            .map(|failure| format!("The answer failed: {}", failure.message)),
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

/// The box for the next message, with the provider it goes to, or Stop
/// while an answer comes in.
fn draw_composer(
    view: &mut View,
    talk: u32,
    state: &State,
    providers: &[Provider],
    messages: &[Message],
    busy: bool,
) {
    let composer = view.composer(talk, COMPOSER, "Message", &state.draft, "Send");
    if busy {
        view.stop(composer, "Stop");
    }
    if providers.is_empty() {
        return;
    }
    let choices: Vec<(&str, &str)> = providers
        .iter()
        .map(|provider| (provider.id.as_str(), provider.name.as_str()))
        .collect();
    let chosen = chosen_provider(state, providers, messages);
    view.choice_input(composer, PROVIDER, "Provider", &choices, chosen.as_deref());
}

/// The provider the next message goes to: the one you picked last, or the
/// one that wrote the chat's last answer, or the first.
fn chosen_provider(state: &State, providers: &[Provider], messages: &[Message]) -> Option<String> {
    let known = |id: &str| providers.iter().any(|provider| provider.id == id);
    state
        .provider
        .clone()
        .filter(|id| known(id))
        .or_else(|| {
            messages
                .iter()
                .rev()
                .filter_map(|message| message.provider.clone())
                .find(|id| known(id))
        })
        .or_else(|| providers.first().map(|provider| provider.id.clone()))
}

/// The sidebar: New chat and every chat, the one changed last first.
pub(crate) fn sidebar(state: &State) -> Vec<Node> {
    let mut view = View::panel("ai-chats", Some("Chats"));
    view.button(View::ROOT, NEW_CHAT_BUTTON, "New chat");
    match ai::chats() {
        Ok(chats) if !chats.is_empty() => {
            let list = view.list(View::ROOT, CHATS, "Chats");
            view.open_on_click(list);
            for chat in &chats {
                let item = view.item(list, &chat.id, &chat.title, None);
                if state
                    .renaming
                    .as_ref()
                    .is_some_and(|(renaming, _)| renaming == &chat.id)
                {
                    view.name_field(item, CHAT_NAME, "Chat name", &chat.title);
                } else {
                    draw_chat_menu(&mut view, item, chat);
                }
            }
            // A new chat isn't listed until its first message goes out.
            if let Some(chat) = &state.chat
                && chats.iter().any(|known| &known.id == chat)
            {
                view.select(list, chat);
            }
        }
        Ok(_) => {}
        Err(error) => {
            view.text(View::ROOT, "chats-problem", &reason(&error));
        }
    }
    view.into_nodes()
}

/// Something you did on the page or in its sidebar section.
pub(crate) fn handle(state: &mut State, event: Event) {
    match event {
        Event::Pressed(node) if node == OPEN_AI_SETTINGS => {
            if let Err(error) = navigation::open_settings(SettingsSection::Ai) {
                state.notice = Some(reason(&error));
            }
        }
        Event::Pressed(node) if node == NEW_CHAT_BUTTON => {
            state.chat = None;
            state.notice = None;
            state.renaming = None;
        }
        // The message box's stop button.
        Event::Pressed(node) if node == COMPOSER => {
            let run = state.chat.as_ref().and_then(|chat| state.runs.get(chat));
            if let Some(run) = run
                && let Err(error) = ai::stop(run)
            {
                state.notice = Some(reason(&error));
            }
        }
        Event::Pressed(node) if node == RESUME => resume(state),
        Event::Chosen(node) => {
            let chosen = |action: &str| {
                node.strip_prefix(action)
                    .and_then(|rest| rest.split_once(':'))
                    .map(|(chat, revision)| (chat.to_owned(), revision.to_owned()))
            };
            if let Some(chat) = chosen(RENAME) {
                state.renaming = Some(chat);
            } else if let Some((chat, revision)) = chosen(KEEP) {
                keep(state, &chat, &revision, Retention::Permanent);
            } else if let Some((chat, revision)) = chosen(EPHEMERAL) {
                keep(state, &chat, &revision, Retention::Ephemeral);
            }
        }
        Event::Named(naming) if naming.node == CHAT_NAME => rename(state, &naming.name),
        Event::Cancelled(node) if node == CHAT_NAME => state.renaming = None,
        Event::Selected(target) | Event::Activated(target) if target.node == CHATS => {
            state.chat = Some(target.item);
            state.notice = None;
        }
        Event::Changed(field) if field.node == PROVIDER => {
            if let Value::Choice(Some(provider)) = field.value {
                state.provider = Some(provider);
            }
        }
        Event::Submitted(submission) if submission.node == COMPOSER => {
            let mut message = String::new();
            for field in submission.fields {
                match field.value {
                    Value::Multiline(text) if field.node == COMPOSER => message = text,
                    Value::Choice(Some(provider)) if field.node == PROVIDER => {
                        state.provider = Some(provider);
                    }
                    _ => {}
                }
            }
            send(state, message);
        }
        _ => {}
    }
}

/// Give the chat you rename the name `title`. A chat that changed
/// elsewhere since you chose Rename keeps that change, and the page says
/// so.
fn rename(state: &mut State, title: &str) {
    let Some((chat, revision)) = state.renaming.take() else {
        return;
    };
    state.notice = ai::rename_chat(&chat, title, &revision)
        .err()
        .map(|error| reason(&error));
}

/// Keep the chat `chat` as `retention` says, unless it changed elsewhere
/// since its `chat.md` held `revision`.
fn keep(state: &mut State, chat: &str, revision: &str, retention: Retention) {
    state.notice = ai::set_retention(chat, retention, revision)
        .err()
        .map(|error| reason(&error));
}

/// Answer your last message in the open chat again, with the provider you
/// picked.
fn resume(state: &mut State) {
    let Some(chat) = state.chat.clone() else {
        return;
    };
    state.notice = None;
    let providers = ai::providers().unwrap_or_default();
    let messages = ai::read_chat(&chat)
        .map(|(_, messages)| messages)
        .unwrap_or_default();
    let Some(provider) = chosen_provider(state, &providers, &messages) else {
        return;
    };
    match ai::resume(&chat, &provider, &offered_tools()) {
        Ok(run) => {
            state.runs.insert(chat, run.id);
            state.provider = Some(provider);
        }
        Err(error) => state.notice = Some(reason(&error)),
    }
}

/// Send `message` to the provider you picked, in the open chat or in a new
/// one, offering the tools of the plugins on in the workspace. A message
/// that doesn't go out comes back to the message box.
fn send(state: &mut State, message: String) {
    if message.trim().is_empty() {
        return;
    }
    state.notice = None;
    let providers = ai::providers().unwrap_or_default();
    let messages = state
        .chat
        .as_ref()
        .and_then(|chat| ai::read_chat(chat).ok())
        .map(|(_, messages)| messages)
        .unwrap_or_default();
    let Some(provider) = chosen_provider(state, &providers, &messages) else {
        state.draft = message;
        return;
    };
    let chat = match state.chat.clone() {
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
    let request = Request {
        chat: chat.clone(),
        provider: provider.clone(),
        message: message.clone(),
        tools: offered_tools(),
    };
    match ai::send(&request) {
        Ok(run) => {
            if !run.message_written {
                state.unsent.insert(run.id.clone(), message);
            }
            state.runs.insert(chat, run.id);
            state.provider = Some(provider);
            state.draft.clear();
        }
        Err(error) => {
            state.notice = Some(reason(&error));
            state.draft = message;
        }
    }
}
