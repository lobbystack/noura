//! The AI page and its sidebar section: the chats, each with a menu to
//! rename it or have it removed after 30 days, the open chat's messages,
//! the answer streaming in, Stop, Resume, and the message box.

use plugin_sdk::{
    Event, Node, Value, View,
    ai::{
        self, Chat, Message, MessageKind, MessageStatus, Provider, Request, Retention, Run,
        RunStatus,
    },
};

use crate::{State, reason};

/// The title a chat has until its first message names it.
const NEW_CHAT: &str = "New chat";

/// The node IDs events name.
const NEW_CHAT_BUTTON: &str = "new-chat";
const CHATS: &str = "chats";
const COMPOSER: &str = "composer";
const PROVIDER: &str = "provider";
const STOP: &str = "stop";
const RESUME: &str = "resume";
const CHAT_NAME: &str = "chat-name";

/// The menu items of a chat in the sidebar, each followed by the chat's
/// ID.
const RENAME: &str = "rename:";
const KEEP: &str = "keep:";
const EPHEMERAL: &str = "ephemeral:";

/// The page: the open chat's messages, then Stop while an answer comes in
/// or the message box.
pub(crate) fn render(state: &mut State) -> Vec<Node> {
    hand_over_tool_calls(state);
    let mut view = View::panel("ai", Some("AI"));
    if let Some(notice) = &state.notice {
        view.text(View::ROOT, "notice", notice);
    }
    let providers = match ai::providers() {
        Ok(providers) => providers,
        Err(error) => {
            view.text(View::ROOT, "providers-problem", &reason(&error));
            return view.into_nodes();
        }
    };
    let mut chat = None;
    let mut messages = Vec::new();
    match state.chat.clone().map(|id| ai::read_chat(&id)) {
        None => {
            view.text(View::ROOT, "title", NEW_CHAT);
        }
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
            view.text(View::ROOT, "title", NEW_CHAT);
            view.text(View::ROOT, "chat-problem", &reason(&error));
        }
    }
    let run = state
        .chat
        .as_ref()
        .and_then(|chat| state.runs.get(chat))
        .and_then(|id| ai::read_run(id).ok());
    let busy = run.as_ref().is_some_and(busy);
    if let Some(chat) = &chat {
        draw_title(&mut view, chat);
    }
    for message in &messages {
        draw_message(&mut view, message, run.as_ref(), &providers);
    }
    if let Some(run) = &run {
        // A message Noura didn't write to the chat, as when you said no or
        // the write failed, comes back to the message box.
        if run.message_written {
            state.unsent.remove(&run.id);
        } else if !busy && let Some(message) = state.unsent.remove(&run.id) {
            state.draft = message;
            state.sent += 1;
        }
        draw_run_status(&mut view, run, &providers, state);
    }
    if busy {
        view.button(View::ROOT, STOP, "Stop");
    } else if providers.is_empty() {
        view.text(
            View::ROOT,
            "no-providers",
            "Add an AI provider in Settings, under AI providers, to start a chat.",
        );
    } else {
        if stopped_before_answering(&messages) {
            view.button(View::ROOT, RESUME, "Resume");
        }
        draw_composer(&mut view, state, &providers, &messages);
    }
    view.into_nodes()
}

/// Whether a run is still at work: Noura asks you something, or the
/// answer streams in.
fn busy(run: &Run) -> bool {
    matches!(
        run.status,
        RunStatus::Streaming | RunStatus::WaitingForConsent | RunStatus::WaitingForApproval
    )
}

/// Hand each run's tool calls to Noura once the model asked for them.
/// Noura asks you about each call in turn and runs the ones you allow,
/// then the model answers again in the same run.
fn hand_over_tool_calls(state: &mut State) {
    let mut problem = None;
    for id in state.runs.values() {
        let Ok(run) = ai::read_run(id) else {
            continue;
        };
        if run.status == RunStatus::Completed
            && !run.tool_calls.is_empty()
            && let Err(error) = ai::run_tools(id)
        {
            problem = Some(reason(&error));
        }
    }
    if problem.is_some() {
        state.notice = problem;
    }
}

/// Whether Noura stopped before your last message got its answer, as
/// after a crash or a quit: the chat's last message is interrupted, with
/// no code that says a plugin went.
fn stopped_before_answering(messages: &[Message]) -> bool {
    messages
        .iter()
        .any(|message| message.kind == MessageKind::User)
        && messages.last().is_some_and(|message| {
            message.status == MessageStatus::Interrupted && message.error_code.is_none()
        })
}

/// The chat's title, and how long Noura keeps it when that isn't until
/// you delete it.
fn draw_title(view: &mut View, chat: &Chat) {
    view.text(View::ROOT, "title", &chat.title);
    if chat.retention == Retention::Ephemeral {
        view.text(
            View::ROOT,
            "retention",
            "Noura moves this chat to the trash 30 days after it started.",
        );
    }
}

/// The menu of a chat in the sidebar: rename it, or choose how long Noura
/// keeps it.
fn draw_chat_menu(view: &mut View, item: u32, chat: &Chat) {
    let menu = view.menu(item, &format!("menu:{}", chat.id), "Chat options");
    view.menu_item(menu, &format!("{RENAME}{}", chat.id), "Rename", true);
    view.separator(menu, &format!("separator:{}", chat.id));
    view.menu_item(
        menu,
        &format!("{KEEP}{}", chat.id),
        "Keep until I delete it",
        chat.retention != Retention::Permanent,
    );
    view.menu_item(
        menu,
        &format!("{EPHEMERAL}{}", chat.id),
        "Remove after 30 days",
        chat.retention != Retention::Ephemeral,
    );
}

/// One message of the chat. The answer that streams in shows the text
/// that arrived so far.
fn draw_message(view: &mut View, message: &Message, run: Option<&Run>, providers: &[Provider]) {
    match message.kind {
        MessageKind::User => {
            let panel = view.add_panel(View::ROOT, &message.id, Some("You"));
            view.text(panel, &format!("{}-author", message.id), "You");
            view.markdown(panel, &format!("{}-text", message.id), &message.content);
        }
        MessageKind::Assistant => {
            let panel = view.add_panel(View::ROOT, &message.id, Some("Answer"));
            let provider = message.provider.as_deref().unwrap_or_default();
            let name = providers
                .iter()
                .find(|known| known.id == provider)
                .map_or(provider, |known| known.name.as_str());
            let model = message.model.as_deref().unwrap_or_default();
            view.text(
                panel,
                &format!("{}-author", message.id),
                &format!("{name}, {model}"),
            );
            let streaming = run.filter(|run| run.message.as_deref() == Some(message.id.as_str()));
            let text = match (message.status, streaming) {
                (MessageStatus::InProgress, Some(run)) => run.text.as_str(),
                _ => message.content.as_str(),
            };
            if !text.is_empty() {
                view.markdown(panel, &format!("{}-text", message.id), text);
            }
            let note = match message.status {
                MessageStatus::InProgress if streaming.is_none() => Some("Not finished."),
                MessageStatus::InProgress => None,
                MessageStatus::Completed => None,
                MessageStatus::Cancelled => Some("Stopped."),
                MessageStatus::Interrupted
                    if message.error_code.as_deref() == Some("plugin-disabled") =>
                {
                    Some("You turned off a plugin this answer used.")
                }
                MessageStatus::Interrupted => Some("Noura stopped before this answer finished."),
                MessageStatus::Failed => Some("This answer failed."),
            };
            if let Some(note) = note {
                view.text(panel, &format!("{}-status", message.id), note);
            }
        }
        MessageKind::ToolCall => {
            let tool = message.tool_name.as_deref().unwrap_or("a tool");
            let panel = view.add_panel(View::ROOT, &message.id, Some("Tool call"));
            view.text(
                panel,
                &format!("{}-tool", message.id),
                &format!("Tool call: {tool}"),
            );
            let note = match message.status {
                MessageStatus::InProgress => Some("Waiting to run."),
                MessageStatus::Completed | MessageStatus::Cancelled => None,
                MessageStatus::Interrupted
                    if message.error_code.as_deref() == Some("plugin-disabled") =>
                {
                    Some("You turned off the plugin that offers this tool, so the call didn't run.")
                }
                MessageStatus::Interrupted => {
                    Some("Noura stopped before this call finished, and won't run it on its own.")
                }
                MessageStatus::Failed => Some("This call failed."),
            };
            if let Some(note) = note {
                view.text(panel, &format!("{}-status", message.id), note);
            }
        }
        MessageKind::ToolResult => {
            let denied = serde_json::from_str::<serde_json::Value>(&message.content)
                .is_ok_and(|result| result["denied"] == true);
            let line = if denied {
                "You didn't allow this call, so it didn't run.".to_owned()
            } else {
                format!("Result: {}", message.content)
            };
            view.text(View::ROOT, &message.id, &line);
        }
        MessageKind::ContextSummary => {
            let panel = view.add_panel(View::ROOT, &message.id, Some("Summary"));
            view.text(
                panel,
                &format!("{}-about", message.id),
                "Noura summarized the messages above to keep this chat within the model's \
                 context.",
            );
            view.markdown(panel, &format!("{}-text", message.id), &message.content);
        }
    }
}

/// What the run of the last message you sent says, when the messages
/// don't: it waits for you, you said no, or it ended early.
fn draw_run_status(view: &mut View, run: &Run, providers: &[Provider], state: &State) {
    let name = providers
        .iter()
        .find(|known| known.id == run.provider)
        .map_or(run.provider.as_str(), |known| known.name.as_str());
    let line = match run.status {
        RunStatus::WaitingForConsent => Some(format!(
            "Noura is asking you whether {name} may receive this workspace's content."
        )),
        RunStatus::WaitingForApproval => {
            Some("Noura is asking you whether to run a tool the model wants to use.".to_owned())
        }
        RunStatus::Denied => Some(format!(
            "You didn't let {name} receive this workspace's content, so Noura sent nothing."
        )),
        RunStatus::Failed => run
            .failure
            .as_ref()
            .map(|failure| format!("The answer failed: {}", failure.message)),
        RunStatus::Interrupted => {
            Some("You turned off a plugin this answer relied on, so the answer stopped.".to_owned())
        }
        RunStatus::Streaming | RunStatus::Completed | RunStatus::Cancelled => None,
    };
    if let Some(line) = line {
        view.text(View::ROOT, &format!("run-{}", state.sent), &line);
    }
}

/// The box for the next message and the provider it goes to.
fn draw_composer(view: &mut View, state: &State, providers: &[Provider], messages: &[Message]) {
    let form = view.form(View::ROOT, COMPOSER, "Message", "Send");
    let input = format!("message-{}", state.sent);
    view.input(
        form,
        &input,
        "Message",
        Value::Multiline(state.draft.clone()),
    );
    view.require(form, &input);
    let choices: Vec<(&str, &str)> = providers
        .iter()
        .map(|provider| (provider.id.as_str(), provider.name.as_str()))
        .collect();
    let chosen = chosen_provider(state, providers, messages);
    view.choice_input(form, PROVIDER, "Provider", &choices, chosen.as_deref());
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
                let detail =
                    (chat.retention == Retention::Ephemeral).then_some("Removed after 30 days");
                let item = view.item(list, &chat.id, &chat.title, detail);
                if state.renaming.as_deref() == Some(chat.id.as_str()) {
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
        Ok(_) => {
            view.text(View::ROOT, "no-chats", "No chats yet.");
        }
        Err(error) => {
            view.text(View::ROOT, "chats-problem", &reason(&error));
        }
    }
    view.into_nodes()
}

/// Something you did on the page or in its sidebar section.
pub(crate) fn handle(state: &mut State, event: Event) {
    match event {
        Event::Pressed(node) if node == NEW_CHAT_BUTTON => {
            state.chat = None;
            state.notice = None;
            state.renaming = None;
        }
        Event::Pressed(node) if node == STOP => {
            let run = state.chat.as_ref().and_then(|chat| state.runs.get(chat));
            if let Some(run) = run
                && let Err(error) = ai::stop(run)
            {
                state.notice = Some(reason(&error));
            }
        }
        Event::Pressed(node) if node == RESUME => resume(state),
        Event::Chosen(node) => {
            if let Some(chat) = node.strip_prefix(RENAME) {
                state.renaming = Some(chat.to_owned());
            } else if let Some(chat) = node.strip_prefix(KEEP) {
                keep(state, chat, Retention::Permanent);
            } else if let Some(chat) = node.strip_prefix(EPHEMERAL) {
                keep(state, chat, Retention::Ephemeral);
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
                    Value::Multiline(text) if field.node.starts_with("message-") => message = text,
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
/// elsewhere keeps that change, and the page says so.
fn rename(state: &mut State, title: &str) {
    let Some(id) = state.renaming.take() else {
        return;
    };
    state.notice = ai::read_chat(&id)
        .and_then(|(chat, _)| ai::rename_chat(&chat.id, title, &chat.revision))
        .err()
        .map(|error| reason(&error));
}

/// Keep the chat `id` as `retention` says.
fn keep(state: &mut State, id: &str, retention: Retention) {
    state.notice = ai::read_chat(id)
        .and_then(|(chat, _)| ai::set_retention(&chat.id, retention, &chat.revision))
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
    let tools = ai::workspace_tools().unwrap_or_default();
    match ai::resume(&chat, &provider, &tools) {
        Ok(run) => {
            state.runs.insert(chat, run.id);
            state.provider = Some(provider);
        }
        Err(error) => state.notice = Some(reason(&error)),
    }
}

/// Send `message` to the provider you picked, in the open chat or in a new
/// one, offering the tools of the plugins on in the workspace.
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
        tools: ai::workspace_tools().unwrap_or_default(),
    };
    match ai::send(&request) {
        Ok(run) => {
            if !run.message_written {
                state.unsent.insert(run.id.clone(), message);
            }
            state.runs.insert(chat, run.id);
            state.provider = Some(provider);
            state.draft.clear();
            state.sent += 1;
        }
        Err(error) => {
            state.notice = Some(reason(&error));
            state.draft = message;
            state.sent += 1;
        }
    }
}
