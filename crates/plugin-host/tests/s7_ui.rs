//! Trial S7 UI round trip: the host draws each hello plugin's nodes with
//! egui, AccessKit finds the plugin's button by its label, and the click goes
//! back to the plugin by node id.
//!
//! The drawing code lives here because `plugin-host` stays free of UI code.
//! Ticket 24 builds the real widget kit in `app-ui`.

mod support;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use plugin_host::trial::{Node, NodeKind, PluginInstance, TrialHost};

/// A plugin's nodes as last rendered, and the node ids sent back to it.
struct PluginView {
    instance: PluginInstance,
    nodes: Vec<Node>,
    sent_events: Vec<String>,
}

impl PluginView {
    fn new(mut instance: PluginInstance) -> Self {
        let nodes = instance.render().expect("render");
        Self {
            instance,
            nodes,
            sent_events: Vec::new(),
        }
    }

    fn show(&mut self, ui: &mut egui::Ui) {
        let mut pressed = None;
        draw_node(ui, &self.nodes, 0, 0, &mut pressed);
        if let Some(node_id) = pressed {
            self.instance.handle_event(&node_id).expect("event");
            self.sent_events.push(node_id);
            self.nodes = self.instance.render().expect("render");
        }
    }
}

fn draw_node(
    ui: &mut egui::Ui,
    nodes: &[Node],
    index: u32,
    depth: usize,
    pressed: &mut Option<String>,
) {
    // A child index past the end or a cycle back up the list draws nothing.
    let Some(node) = nodes.get(index as usize) else {
        return;
    };
    if depth > nodes.len() {
        return;
    }
    match &node.kind {
        NodeKind::Panel => {
            ui.vertical(|ui| {
                for &child in &node.children {
                    draw_node(ui, nodes, child, depth + 1, pressed);
                }
            });
        }
        NodeKind::Text(text) => {
            ui.label(text);
        }
        NodeKind::Button(label) => {
            if ui.button(label).clicked() {
                *pressed = Some(node.id.clone());
            }
        }
    }
}

#[test]
fn clicking_a_plugin_button_redraws_the_plugins_new_count() {
    let Some(plugins) = support::hello_plugins() else {
        return;
    };

    for backend in support::backends() {
        let host = TrialHost::new(backend).expect("host starts");
        for plugin in &plugins {
            let compiled = host.compile(&plugin.bytes).expect("plugin compiles");
            let view = PluginView::new(host.instantiate(&compiled).expect("plugin instantiates"));
            let mut harness =
                Harness::new_ui_state(|ui, view: &mut PluginView| view.show(ui), view);

            harness.get_by_label("Count: 0");
            harness.get_by_label("Add one").click();
            harness.run();

            harness.get_by_label("Count: 1");
            assert_eq!(
                harness.state().sent_events,
                ["add-one"],
                "{} plugin on {backend:?}",
                plugin.name
            );
        }
    }
}
