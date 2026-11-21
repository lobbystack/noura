//! The Orbs page's sidebar section: New Orb, then every Orb, the default
//! Orb first, each with its avatar and a dot when it works, waits or
//! failed. Selecting one opens its thread.

use plugin_sdk::{
    Event, Node, View,
    ai::{self, Orb, OrbRunStatus},
};

use crate::{State, look, reason};

/// The node IDs events name.
const NEW_ORB: &str = "new-orb";
const ORB_NAME: &str = "orb-new-name";
pub(crate) const ORBS: &str = "orbs";

/// Whether the Orb's last run failed.
fn failed(orb: &Orb) -> bool {
    orb.id != look::DEFAULT_ORB
        && ai::orb_runs(&orb.id, 1)
            .ok()
            .and_then(|runs| runs.into_iter().next())
            .is_some_and(|run| run.status == OrbRunStatus::Failed)
}

pub(crate) fn sidebar(state: &State) -> Vec<Node> {
    let mut view = View::panel("ai-orbs", Some("Orbs"));
    view.button(View::ROOT, NEW_ORB, "New Orb");
    if state.naming {
        view.name_field(View::ROOT, ORB_NAME, "Orb name", "");
    }
    if let Some(notice) = &state.sidebar_notice {
        view.text(View::ROOT, "orbs-notice", notice);
    }
    match ai::orbs() {
        Ok(orbs) => {
            let list = view.list(View::ROOT, ORBS, "Orbs");
            view.open_on_click(list);
            for orb in &orbs {
                let item = view.item(list, &orb.id, &orb.title, None);
                view.avatar(item, look::avatar(orb));
                if let Some((label, color)) = look::status(orb, failed(orb)) {
                    view.status(item, label, color);
                }
            }
            let shown = state.orb.as_deref().unwrap_or(look::DEFAULT_ORB);
            if orbs.iter().any(|orb| orb.id == shown) {
                view.select(list, shown);
            }
        }
        Err(error) => {
            view.text(View::ROOT, "orbs-problem", &reason(&error));
        }
    }
    view.into_nodes()
}

/// Handle `event` when it's the sidebar's, and say whether it was.
pub(crate) fn handle(state: &mut State, event: &Event) -> bool {
    match event {
        Event::Pressed(node) if node == NEW_ORB => {
            state.naming = true;
            state.sidebar_notice = None;
        }
        Event::Named(naming) if naming.node == ORB_NAME => {
            state.naming = false;
            match ai::create_orb(&naming.name) {
                Ok(orb) => {
                    state.orb = Some(orb.id);
                    state.notice = None;
                    state.sidebar_notice = None;
                    // A new Orb's model and effort wait in its panel.
                    state.panel = true;
                }
                Err(error) => state.sidebar_notice = Some(reason(&error)),
            }
        }
        Event::Cancelled(node) if node == ORB_NAME => state.naming = false,
        Event::Selected(target) | Event::Activated(target) if target.node == ORBS => {
            state.orb = (target.item != look::DEFAULT_ORB).then(|| target.item.clone());
            state.notice = None;
            state.panel_notice = None;
            state.renaming = None;
        }
        _ => return false,
    }
    true
}
