//! The Orb's side panel, which the pill with its name opens: its avatar
//! and name, the model it asks and how hard that model reasons, then its
//! recent runs and its triggers. The default Orb has no file, so its name
//! stays, and its plain chats take the place of runs and triggers, each
//! with a menu to rename it or choose how long Noura keeps it.

use plugin_sdk::{
    EditableStyle, Event, Value, View,
    ai::{self, Chat, Effort, Model, Orb, OrbChanges, OrbStart, Retention},
    clock,
};

use crate::{State, look, page, reason, triggers};

/// The node IDs events name.
const PANEL: &str = "orb-panel";
const NAME: &str = "orb-name";
const MODEL: &str = "orb-model";
const EFFORT: &str = "orb-effort";
const TRIGGERS: &str = "orb-triggers";
const ADD_TRIGGER: &str = "add-trigger";
const NEW_CHAT: &str = "new-chat";
pub(crate) const CHATS: &str = "chats";
const CHAT_NAME: &str = "chat-name";

/// The menu item that removes a trigger, followed by the trigger's index.
const REMOVE_TRIGGER: &str = "remove-trigger:";

/// The menu items of a chat, each followed by the chat's ID, a colon and
/// the revision of its `chat.md` the menu showed, which the change
/// carries: a chat that changed elsewhere since keeps that change.
const RENAME: &str = "rename:";
const KEEP: &str = "keep:";
const EPHEMERAL: &str = "ephemeral:";

/// How wide the panel is, in points.
const WIDTH: u32 = 320;

/// How many runs the panel lists.
const RUNS_SHOWN: u32 = 8;

fn no_changes() -> OrbChanges {
    OrbChanges {
        title: None,
        instructions: None,
        avatar: None,
        color: None,
        provider: None,
        model: None,
        effort: None,
        tools: None,
        triggers: None,
    }
}

/// The model the Orb asks: its own when it names one, or its provider's,
/// as an entry of `models`, which gains it when Noura doesn't list it.
fn chosen_model(orb: &Orb, models: &mut Vec<Model>) -> Option<Model> {
    let providers = ai::providers().unwrap_or_default();
    let provider = look::provider_of(orb, &providers)?;
    let id = orb.model.clone().unwrap_or_else(|| provider.model.clone());
    if let Some(known) = models
        .iter()
        .find(|model| model.provider == provider.id && model.id == id)
    {
        return Some(known.clone());
    }
    let named = Model {
        provider: provider.id,
        name: id.clone(),
        id,
        efforts: Vec::new(),
        default_effort: None,
    };
    models.push(named.clone());
    Some(named)
}

/// Draw the panel for `orb` as the view's last node. `chat` is the plain
/// chat the page shows for the default Orb.
pub(crate) fn draw(view: &mut View, state: &State, orb: &Orb, chat: Option<&Chat>) {
    let panel = view.side_panel(PANEL, &orb.title, WIDTH);
    view.large_avatar(panel, "orb-avatar", look::avatar(orb));
    let default = orb.id == look::DEFAULT_ORB;
    if default {
        view.section_heading(panel, NAME, &orb.title, None);
    } else {
        view.editable(panel, NAME, "Name", &orb.title, EditableStyle::Title, false);
    }
    if let Some(problem) = &orb.problem {
        view.text(
            panel,
            "orb-problem",
            &format!("Noura can't run it: {problem}."),
        );
    }
    let mut models = ai::models().unwrap_or_default();
    if let Some(chosen) = chosen_model(orb, &mut models) {
        let choices: Vec<(String, String)> = models
            .iter()
            .map(|model| {
                (
                    look::model_id(&model.provider, &model.id),
                    label(model, &models),
                )
            })
            .collect();
        let choices: Vec<(&str, &str)> = choices
            .iter()
            .map(|(id, label)| (id.as_str(), label.as_str()))
            .collect();
        let chosen_id = look::model_id(&chosen.provider, &chosen.id);
        view.choice_input(panel, MODEL, "Model", &choices, Some(&chosen_id));
        if !chosen.efforts.is_empty() {
            let efforts: Vec<(&str, &str)> = chosen
                .efforts
                .iter()
                .map(|effort| (look::effort_id(*effort), look::effort_label(*effort)))
                .collect();
            let effort = orb
                .effort
                .filter(|effort| chosen.efforts.contains(effort))
                .or(chosen.default_effort);
            view.choice_input(
                panel,
                EFFORT,
                "Effort",
                &efforts,
                effort.map(look::effort_id),
            );
        }
    }
    if let Some(notice) = &state.panel_notice {
        view.text(panel, "panel-notice", notice);
    }
    if default {
        draw_chats(view, state, panel, chat);
    } else {
        draw_activity(view, panel, orb);
    }
}

/// A model's name in the menu, with its provider's when another provider
/// offers a model of the same name.
fn label(model: &Model, models: &[Model]) -> String {
    let shared = models
        .iter()
        .any(|other| other.name == model.name && other.provider != model.provider);
    if !shared {
        return model.name.clone();
    }
    let providers = ai::providers().unwrap_or_default();
    let provider = providers
        .iter()
        .find(|provider| provider.id == model.provider)
        .map_or(model.provider.as_str(), |provider| provider.name.as_str());
    format!("{} ({provider})", model.name)
}

/// The Orb's recent runs, with what started each, how it ended and when,
/// then its triggers in plain words, each with a menu to remove it, and a
/// field to add one.
fn draw_activity(view: &mut View, panel: u32, orb: &Orb) {
    let runs = ai::orb_runs(&orb.id, RUNS_SHOWN).unwrap_or_default();
    if !runs.is_empty() {
        view.section_heading(panel, "activity", "Activity", None);
        let now = clock::now().instant;
        let list = view.list(panel, "orb-runs", "Runs");
        for run in &runs {
            let started = match run.started_by {
                OrbStart::You => "You",
                OrbStart::Schedule => "Schedule",
                OrbStart::Event => "Event",
            };
            let status = look::run_status(run.status, run.quiet);
            let detail = match look::ago(&run.started, &now) {
                Some(ago) => format!("{status} · {ago}"),
                None => status.to_owned(),
            };
            view.item(list, &run.id, started, Some(&detail));
        }
    }
    view.section_heading(panel, "triggers", "Triggers", None);
    if !orb.triggers.is_empty() {
        let list = view.list(panel, TRIGGERS, "Triggers");
        for (index, trigger) in orb.triggers.iter().enumerate() {
            let item = view.item(
                list,
                &format!("trigger-{index}"),
                &triggers::words(trigger),
                None,
            );
            let menu = view.menu(item, &format!("trigger-menu-{index}"), "Trigger options");
            view.menu_item(menu, &format!("{REMOVE_TRIGGER}{index}"), "Remove", true);
        }
    }
    view.editable(
        panel,
        ADD_TRIGGER,
        "Add trigger",
        "",
        EditableStyle::Add,
        false,
    );
}

/// The default Orb's plain chats, the one the page shows selected, with
/// New chat above them.
fn draw_chats(view: &mut View, state: &State, panel: u32, shown: Option<&Chat>) {
    let heading = view.section_heading(panel, "chats-heading", "Chats", None);
    view.button(heading, NEW_CHAT, "New chat");
    let chats: Vec<Chat> = ai::chats()
        .unwrap_or_default()
        .into_iter()
        .filter(|chat| chat.orb.is_none())
        .collect();
    if chats.is_empty() {
        return;
    }
    let list = view.list(panel, CHATS, "Chats");
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
            draw_chat_menu(view, item, chat);
        }
    }
    // A new chat isn't listed until its first message goes out.
    if let Some(shown) = shown
        && chats.iter().any(|known| known.id == shown.id)
    {
        view.select(list, &shown.id);
    }
}

/// The menu of a chat: rename it, or choose how long Noura keeps it.
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

/// Something you did in the panel.
pub(crate) fn handle(state: &mut State, event: Event) {
    match event {
        Event::Changed(field) if field.node == MODEL => {
            if let Value::Choice(Some(id)) = field.value
                && let Some((provider, model)) = look::model_named(&id)
            {
                choose_model(state, &provider, &model);
            }
        }
        Event::Changed(field) if field.node == EFFORT => {
            if let Value::Choice(Some(id)) = field.value
                && let Some(effort) = look::effort_named(&id)
            {
                change(state, |_| OrbChanges {
                    effort: Some(Some(effort)),
                    ..no_changes()
                });
            }
        }
        Event::Changed(field) if field.node == NAME => {
            if let Value::Text(title) = field.value
                && !title.trim().is_empty()
            {
                change(state, |_| OrbChanges {
                    title: Some(title.trim().to_owned()),
                    ..no_changes()
                });
            }
        }
        Event::Changed(field) if field.node == ADD_TRIGGER => {
            if let Value::Text(trigger) = field.value {
                change(state, |orb| {
                    let mut triggers = orb.triggers.clone();
                    triggers.push(trigger.trim().to_owned());
                    OrbChanges {
                        triggers: Some(triggers),
                        ..no_changes()
                    }
                });
            }
        }
        Event::Chosen(node) if node.starts_with(REMOVE_TRIGGER) => {
            let index = node[REMOVE_TRIGGER.len()..].parse::<usize>().ok();
            change(state, |orb| {
                let mut triggers = orb.triggers.clone();
                if let Some(index) = index.filter(|index| *index < triggers.len()) {
                    triggers.remove(index);
                }
                OrbChanges {
                    triggers: Some(triggers),
                    ..no_changes()
                }
            });
        }
        Event::Pressed(node) if node == NEW_CHAT => {
            state.chat = None;
            state.new_chat = true;
            state.notice = None;
            state.renaming = None;
        }
        Event::Selected(target) | Event::Activated(target) if target.node == CHATS => {
            state.chat = Some(target.item);
            state.new_chat = false;
            state.notice = None;
        }
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
        _ => {}
    }
}

/// Point the open Orb at `model` of `provider`, and drop an effort the new
/// model doesn't take.
fn choose_model(state: &mut State, provider: &str, model: &str) {
    let models = ai::models().unwrap_or_default();
    let efforts: Vec<Effort> = models
        .iter()
        .find(|known| known.provider == provider && known.id == model)
        .map(|known| known.efforts.clone())
        .unwrap_or_default();
    change(state, |orb| OrbChanges {
        provider: Some(provider.to_owned()),
        model: Some(model.to_owned()),
        effort: orb
            .effort
            .filter(|effort| !efforts.contains(effort))
            .map(|_| None),
        ..no_changes()
    });
}

/// Change the open Orb as `changes` says for it as it stands now.
fn change(state: &mut State, changes: impl FnOnce(&Orb) -> OrbChanges) {
    let orbs = ai::orbs().unwrap_or_default();
    let Some(orb) = page::open_orb(state, &orbs) else {
        return;
    };
    state.panel_notice = ai::update_orb(&orb.id, &changes(&orb), &orb.revision)
        .err()
        .map(|error| match error {
            plugin_sdk::Error::RevisionConflict(_) => {
                "The Orb changed elsewhere. Showing the latest version.".to_owned()
            }
            error => reason(&error),
        });
}

/// Give the chat you rename the name `title`. A chat that changed
/// elsewhere since you chose Rename keeps that change, and the panel says
/// so.
fn rename(state: &mut State, title: &str) {
    let Some((chat, revision)) = state.renaming.take() else {
        return;
    };
    state.panel_notice = ai::rename_chat(&chat, title, &revision)
        .err()
        .map(|error| reason(&error));
}

/// Keep the chat `chat` as `retention` says, unless it changed elsewhere
/// since its `chat.md` held `revision`.
fn keep(state: &mut State, chat: &str, revision: &str, retention: Retention) {
    state.panel_notice = ai::set_retention(chat, retention, revision)
        .err()
        .map(|error| reason(&error));
}
