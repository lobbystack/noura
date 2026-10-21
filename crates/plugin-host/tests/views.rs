//! Seam 3: UI as data. A view is a flat node list linked by index, which
//! the host checks before anything draws it, and presses go back to the
//! plugin by node ID.

mod support;

use plugin_host::{Event, PluginError, Services, View, ViewKind, ViewNode};
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
    let (instance, _folders) = plugins::running(&probe, &VIEWS, None);

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
    let (instance, _folders) = plugins::running(&probe, &VIEWS, None);

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
    let (instance, _folders) = plugins::running(&probe, &VIEWS, None);

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
    let (instance, _folders) = plugins::running(&component, &VIEWS, None);

    let error = instance.render("any").expect_err("the render fails");

    assert!(matches!(error, PluginError::Call(_)), "{error:?}");
    assert_eq!(instance.pages().expect("the plugin still answers"), []);
}

#[test]
fn a_component_that_exports_only_its_view_registers_nothing_else() {
    let component = wat::parse_str(UNKNOWN_KIND).unwrap();
    let (plugin, _folders) = plugins::running(
        &component,
        &[
            "name = \"views\"",
            "name = \"commands\"",
            "name = \"tools\"",
        ],
        None,
    );

    assert_eq!(plugin.pages(), Ok(Vec::new()));
    assert_eq!(
        plugin.commands(),
        Err(PluginError::NotExported {
            interface: "noura:plugin/commands"
        })
    );
    assert_eq!(
        plugin.tools(),
        Err(PluginError::NotExported {
            interface: "noura:plugin/tools"
        })
    );
    assert_eq!(plugin.health().recent_failures, 0);
}

#[test]
fn a_component_that_exports_the_view_interface_without_its_functions_or_types_fails_to_start() {
    for (case, component) in [
        ("missing functions", MISSING_FUNCTIONS),
        ("wrong types", WRONG_TYPES),
    ] {
        let folder = tempfile::tempdir().unwrap();
        let library = tempfile::tempdir().unwrap();
        plugins::write_folder(
            folder.path(),
            &plugins::manifest("probe", "plugin.wasm", &VIEWS),
            "plugin.wasm",
            &wat::parse_str(component).unwrap(),
        );
        let host = plugins::host(library.path());
        host.install(&host.inspect(folder.path()).unwrap()).unwrap();
        let loaded = host.load("probe").unwrap();
        let plugin = host.start(
            &loaded,
            &loaded.manifest().capabilities,
            Services::default(),
        );

        let Err(error) = plugin.pages() else {
            panic!("{case}: the plugin started");
        };
        assert!(
            matches!(error, PluginError::Instantiate(_)),
            "{case}: {error:?}"
        );
    }
}

/// A component exporting `noura:plugin/view@0.1.0` with `pages` alone.
const MISSING_FUNCTIONS: &str = r#"
(component
  (core module $m (func (export "pages")))
  (core instance $i (instantiate $m))
  (func $pages (canon lift (core func $i "pages")))
  (instance $view (export "pages" (func $pages)))
  (export "noura:plugin/view@0.1.0" (instance $view)))
"#;

/// A component exporting `noura:plugin/view@0.1.0` whose functions take
/// and return nothing.
const WRONG_TYPES: &str = r#"
(component
  (core module $m (func (export "f")))
  (core instance $i (instantiate $m))
  (func $f (canon lift (core func $i "f")))
  (instance $view
    (export "pages" (func $f))
    (export "render" (func $f))
    (export "handle-event" (func $f)))
  (export "noura:plugin/view@0.1.0" (instance $view)))
"#;

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
