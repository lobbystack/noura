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
        let pressed = draw_nodes(ui, &self.nodes);
        if let Some(node_id) = pressed {
            self.instance.handle_event(&node_id).expect("event");
            self.sent_events.push(node_id);
            self.nodes = self.instance.render().expect("render");
        }
    }
}

/// How deep panels may nest. A deeper node draws nothing, so a long chain of
/// panels can't exhaust the stack.
const MAX_DEPTH: usize = 32;

/// Draws a plugin's nodes from the root at index 0 and returns the id of the
/// button pressed this frame, if any.
///
/// The node list comes from the plugin, so it is untrusted: a child index may
/// point past the end, back up the list, or at a node another panel already
/// drew. Each node draws at most once, which keeps a frame's work linear in
/// the list's size.
fn draw_nodes(ui: &mut egui::Ui, nodes: &[Node]) -> Option<String> {
    let mut drawn = vec![false; nodes.len()];
    let mut pressed = None;
    draw_node(ui, nodes, 0, 0, &mut drawn, &mut pressed);
    pressed
}

fn draw_node(
    ui: &mut egui::Ui,
    nodes: &[Node],
    index: u32,
    depth: usize,
    drawn: &mut [bool],
    pressed: &mut Option<String>,
) {
    let index = index as usize;
    let (Some(node), Some(seen)) = (nodes.get(index), drawn.get_mut(index)) else {
        return;
    };
    if *seen || depth > MAX_DEPTH {
        return;
    }
    *seen = true;
    match &node.kind {
        NodeKind::Panel => {
            ui.vertical(|ui| {
                for &child in &node.children {
                    draw_node(ui, nodes, child, depth + 1, drawn, pressed);
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

#[test]
fn a_node_list_that_repeats_children_draws_each_node_once() {
    // Each panel lists the next node twice, so drawing every reference would
    // take about 2^30 calls a frame. The last panel lists the button.
    let panels = 30;
    let mut nodes: Vec<Node> = (0..panels)
        .map(|index| Node {
            id: format!("panel-{index}"),
            kind: NodeKind::Panel,
            children: vec![index + 1, index + 1],
        })
        .collect();
    nodes.push(Node {
        id: "only".into(),
        kind: NodeKind::Button("Only button".into()),
        children: Vec::new(),
    });

    let harness = Harness::new_ui(|ui| {
        draw_nodes(ui, &nodes);
    });

    // `get_by_label` fails when the label matches more than one node.
    harness.get_by_label("Only button");
}
