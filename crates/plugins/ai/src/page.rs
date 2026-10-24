//! The AI page and its sidebar section: the chats, the open chat's
//! messages, the answer streaming in, Stop, and the message box.

use plugin_sdk::{
    Event, Node, Value, View,
    ai::{self, Message, MessageKind, MessageStatus, Provider, Request, Run, RunStatus},
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

/// The page: the open chat's messages, then Stop while an answer comes in
/// or the message box.
pub(crate) fn render(state: &mut State) -> Vec<Node> {
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
    let mut messages = Vec::new();
    match state.chat.clone().map(|id| ai::read_chat(&id)) {
        None => {
            view.text(View::ROOT, "title", NEW_CHAT);
        }
        Some(Ok((chat, read))) => {
            view.text(View::ROOT, "title", &chat.title);
            messages = read;
        }
        Some(Err(error)) => {
            // The chat went, as when its folder moved to the trash.
            state.chat = None;
            view.text(View::ROOT, "title", NEW_CHAT);
            view.text(View::ROOT, "chat-problem", &reason(&error));
        }
    }
    let run = state
        .chat
        .as_ref()
        .and_then(|chat| state.runs.get(chat))
        .and_then(|id| ai::read_run(id).ok());
    for message in &messages {
        draw_message(&mut view, message, run.as_ref(), &providers);
    }
    let busy = run.as_ref().is_some_and(|run| {
        matches!(
            run.status,
            RunStatus::Streaming | RunStatus::WaitingForConsent
        )
    });
    if let Some(run) = &run {
        // A message you didn't let go out comes back to the message box.
        if run.status != RunStatus::WaitingForConsent
            && let Some(message) = state.unsent.remove(&run.id)
            && run.status == RunStatus::Denied
        {
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
        draw_composer(&mut view, state, &providers, &messages);
    }
    view.into_nodes()
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
                MessageStatus::Interrupted => Some("Noura stopped before this answer finished."),
                MessageStatus::Failed => Some("This answer failed."),
            };
            if let Some(note) = note {
                view.text(panel, &format!("{}-status", message.id), note);
            }
        }
        MessageKind::ToolCall | MessageKind::ToolResult | MessageKind::ContextSummary => {}
    }
}

/// What the run of the last message you sent says, when the messages
/// don't: it waits for your consent, you said no, or it failed.
fn draw_run_status(view: &mut View, run: &Run, providers: &[Provider], state: &State) {
    let name = providers
        .iter()
        .find(|known| known.id == run.provider)
        .map_or(run.provider.as_str(), |known| known.name.as_str());
    let line = match run.status {
        RunStatus::WaitingForConsent => Some(format!(
            "Noura is asking you whether {name} may receive this workspace's content."
        )),
        RunStatus::Denied => Some(format!(
            "You didn't let {name} receive this workspace's content, so Noura sent nothing."
        )),
        RunStatus::Failed => run
            .failure
            .as_ref()
            .map(|failure| format!("The answer failed: {}", failure.message)),
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
                view.item(list, &chat.id, &chat.title, None);
            }
            if let Some(chat) = &state.chat {
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
        }
        Event::Pressed(node) if node == STOP => {
            let run = state.chat.as_ref().and_then(|chat| state.runs.get(chat));
            if let Some(run) = run
                && let Err(error) = ai::stop(run)
            {
                state.notice = Some(reason(&error));
            }
        }
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

/// Send `message` to the provider you picked, in the open chat or in a new
/// one.
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
        tools: Vec::new(),
    };
    match ai::send(&request) {
        Ok(run) => {
            if run.status == RunStatus::WaitingForConsent {
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
