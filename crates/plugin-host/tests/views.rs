//! Seam 3: UI as data. A view is a flat node list linked by index, which
//! the host checks before anything draws it, and presses go back to the
//! plugin by node ID.

mod support;

use plugin_host::{Event, PluginError, View, ViewKind, ViewNode};
use support::plugins;

const VIEWS: [&str; 1] = ["name = \"views\""];

fn leaf(id: &str, kind: ViewKind) -> ViewNode {
    ViewNode {
        id: id.into(),
        kind,
        children: Vec::new(),
    }
}

#[test]
fn a_flat_node_list_becomes_a_tree_of_panels_text_and_buttons() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (mut instance, _folders) = plugins::running(&probe, &VIEWS, None);

    let view = View::from_nodes(instance.render("probe").unwrap()).expect("the view is valid");

    assert_eq!(
        view.root(),
        &ViewNode {
            id: "root".into(),
            kind: ViewKind::Panel {
                label: Some("Probe".into())
            },
            children: vec![
                leaf("line", ViewKind::Text("Probe is running.".into())),
                leaf(
                    "press",
                    ViewKind::Button {
                        label: "Press".into()
                    }
                ),
            ],
        }
    );
}

#[test]
fn presses_reach_the_plugin_by_node_id() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (mut instance, _folders) = plugins::running(&probe, &VIEWS, None);

    instance
        .handle_event("probe", &Event::Pressed("press".into()))
        .expect("the plugin takes the event");

    let nodes = instance.render("events").unwrap();
    assert_eq!(plugins::texts(&nodes), ["probe pressed press"]);
}

#[test]
fn a_node_list_that_breaks_the_rules_is_refused_with_a_reason() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (mut instance, _folders) = plugins::running(&probe, &VIEWS, None);

    for (case, reason) in [
        ("out-of-range", "child 5"),
        ("cycle", "cycle"),
        ("unlabeled", "label"),
        ("duplicate-id", "same"),
        ("children-of-text", "Only panels"),
        ("empty", "no nodes"),
    ] {
        let nodes = instance.render(&format!("nodes:{case}")).unwrap();
        let error = View::from_nodes(nodes).expect_err(case);
        assert!(error.to_string().contains(reason), "{case}: {error}");
    }
}

#[test]
fn a_node_of_a_kind_the_host_doesnt_know_fails_the_render_and_the_plugin_keeps_running() {
    let component = wat::parse_str(UNKNOWN_KIND).unwrap();
    let (mut instance, _folders) = plugins::running(&component, &VIEWS, None);

    let error = instance.render("any").expect_err("the render fails");

    assert!(matches!(error, PluginError::Call(_)), "{error:?}");
    assert_eq!(instance.pages().expect("the plugin still answers"), []);
}

/// A component exporting `noura:plugin/view@0.1.0` whose `render` returns
/// one node with kind number 9, which `node-kind` doesn't have.
const UNKNOWN_KIND: &str = r#"
(component
  (component $view
    (core module $m
      (memory (export "memory") 1)
      (data (i32.const 8) "\10\00\00\00\01\00\00\00")
      (data (i32.const 16) "\64\00\00\00\04\00\00\00\09")
      (data (i32.const 100) "root")
      (func (export "pages") (result i32) i32.const 200)
      (func (export "render") (param i32 i32) (result i32) i32.const 8)
      (func (export "handle-event") (param i32 i32 i32 i32 i32))
      (func (export "realloc") (param i32 i32 i32 i32) (result i32) i32.const 1024))
    (core instance $i (instantiate $m))
    (type $node-kind' (variant
      (case "panel" (option string)) (case "text" string)
      (case "markdown" string) (case "button" string)))
    (export $node-kind "node-kind" (type $node-kind'))
    (type $node' (record
      (field "id" string) (field "kind" $node-kind) (field "children" (list u32))))
    (export $node "node" (type $node'))
    (type $page' (record (field "id" string) (field "title" string)))
    (export $page "page" (type $page'))
    (type $event' (variant (case "pressed" string)))
    (export $event "event" (type $event'))
    (func $pages (result (list $page))
      (canon lift (core func $i "pages") (memory (core memory $i "memory"))
        (realloc (core func $i "realloc"))))
    (export "pages" (func $pages))
    (func $render (param "page" string) (result (list $node))
      (canon lift (core func $i "render") (memory (core memory $i "memory"))
        (realloc (core func $i "realloc"))))
    (export "render" (func $render))
    (func $handle-event (param "page" string) (param "event" $event)
      (canon lift (core func $i "handle-event") (memory (core memory $i "memory"))
        (realloc (core func $i "realloc"))))
    (export "handle-event" (func $handle-event)))
  (instance $view-instance (instantiate $view))
  (export "noura:plugin/view@0.1.0" (instance $view-instance)))
"#;
