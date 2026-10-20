//! Seam 3 for trial S7: the Rust and TypeScript hello plugins run in the real
//! Wasmtime host.

mod support;

use std::time::{Duration, Instant};

use plugin_host::trial::{Backend, Limits, Node, NodeKind, TrialError, TrialHost};

/// A hand-written component for the trial world. Its memory starts at
/// `pages` pages of 64 KiB, `render` returns an empty list, and
/// `handle-event` never returns.
fn spinning_component(pages: u32) -> String {
    format!(
        r#"
(component
  (core module $m
    (memory (export "memory") {pages})
    (data (i32.const 8) "\00\00\00\00\00\00\00\00")
    (func (export "render") (result i32) i32.const 8)
    (func (export "handle-event") (param i32 i32) (loop $spin (br $spin)))
    (func (export "realloc") (param i32 i32 i32 i32) (result i32) i32.const 64))
  (core instance $i (instantiate $m))
  (type $node-kind' (variant (case "panel") (case "text" string) (case "button" string)))
  (export $node-kind "node-kind" (type $node-kind'))
  (type $node' (record (field "id" string) (field "kind" $node-kind) (field "children" (list u32))))
  (export $node "node" (type $node'))
  (func $render (result (list $node))
    (canon lift (core func $i "render") (memory (core memory $i "memory"))))
  (export "render" (func $render))
  (func $handle-event (param "node-id" string)
    (canon lift (core func $i "handle-event") (memory (core memory $i "memory")) (realloc (core func $i "realloc"))))
  (export "handle-event" (func $handle-event)))
"#
    )
}

fn node(id: &str, kind: NodeKind, children: &[u32]) -> Node {
    Node {
        id: id.into(),
        kind,
        children: children.to_vec(),
    }
}

#[test]
fn each_plugin_renders_a_panel_with_its_count_and_a_button() {
    let Some(plugins) = support::hello_plugins() else {
        return;
    };
    let host = TrialHost::new(Backend::Cranelift).expect("host starts");

    for plugin in plugins {
        let compiled = host.compile(&plugin.bytes).expect("plugin compiles");
        let mut instance = host.instantiate(&compiled).expect("plugin instantiates");

        assert_eq!(
            instance.render().expect("render"),
            vec![
                node("root", NodeKind::Panel, &[1, 2]),
                node("count", NodeKind::Text("Count: 0".into()), &[]),
                node("add-one", NodeKind::Button("Add one".into()), &[]),
            ],
            "{} plugin",
            plugin.name
        );
    }
}

#[test]
fn a_button_event_reaches_the_plugin_by_node_id() {
    let Some(plugins) = support::hello_plugins() else {
        return;
    };
    let host = TrialHost::new(Backend::Cranelift).expect("host starts");

    for plugin in plugins {
        let compiled = host.compile(&plugin.bytes).expect("plugin compiles");
        let mut instance = host.instantiate(&compiled).expect("plugin instantiates");

        instance.handle_event("add-one").expect("event");
        instance
            .handle_event("count")
            .expect("event on a text node");
        instance.handle_event("add-one").expect("event");

        let nodes = instance.render().expect("render");
        assert_eq!(
            nodes[1].kind,
            NodeKind::Text("Count: 2".into()),
            "{} plugin",
            plugin.name
        );
    }
}

#[test]
fn a_plugin_loaded_from_the_hosts_cache_runs_like_a_fresh_compile() {
    let Some(plugins) = support::hello_plugins() else {
        return;
    };
    let cache = tempfile::tempdir().expect("cache directory");

    for backend in support::backends() {
        let host = TrialHost::new(backend).expect("host starts");
        for plugin in &plugins {
            let path = cache
                .path()
                .join(format!("{}-{backend:?}.cwasm", plugin.name));
            let cached = host
                .compile_to_cache(&plugin.bytes, &path)
                .expect("plugin compiles into the cache");

            let compiled = host.load_cached(&cached).expect("cached code loads");
            let mut instance = host.instantiate(&compiled).expect("plugin instantiates");
            instance.handle_event("add-one").expect("event");

            assert_eq!(
                instance.render().expect("render")[1].kind,
                NodeKind::Text("Count: 1".into()),
                "{} plugin on {backend:?}",
                plugin.name
            );
        }
    }
}

#[cfg(feature = "pulley")]
#[test]
fn a_host_refuses_cached_code_that_another_backend_compiled() {
    let cache = tempfile::tempdir().expect("cache directory");
    let cranelift = TrialHost::new(Backend::Cranelift).expect("host starts");
    let pulley = TrialHost::new(Backend::Pulley).expect("host starts");
    let cached = cranelift
        .compile_to_cache(
            spinning_component(1).as_bytes(),
            &cache.path().join("spin.cwasm"),
        )
        .expect("component compiles into the cache");

    assert!(matches!(
        pulley.load_cached(&cached),
        Err(TrialError::Cache(_))
    ));
}

#[test]
fn each_instance_keeps_its_state_in_its_own_store() {
    let Some(plugins) = support::hello_plugins() else {
        return;
    };
    let host = TrialHost::new(Backend::Cranelift).expect("host starts");

    for plugin in plugins {
        let compiled = host.compile(&plugin.bytes).expect("plugin compiles");
        let mut pressed = host.instantiate(&compiled).expect("first instance");
        let mut untouched = host.instantiate(&compiled).expect("second instance");

        pressed.handle_event("add-one").expect("event");

        assert_eq!(
            pressed.render().expect("render")[1].kind,
            NodeKind::Text("Count: 1".into())
        );
        assert_eq!(
            untouched.render().expect("render")[1].kind,
            NodeKind::Text("Count: 0".into()),
            "{} plugin",
            plugin.name
        );
    }
}

#[test]
fn a_plugin_whose_memory_exceeds_the_cap_does_not_start() {
    let limits = Limits {
        memory_bytes: 16 << 20,
        ..Limits::default()
    };

    for backend in support::backends() {
        let host = TrialHost::with_limits(backend, limits).expect("host starts");

        let within = host
            .compile(spinning_component(256).as_bytes())
            .expect("component compiles");
        let mut instance = host.instantiate(&within).expect("16 MiB fits the cap");
        assert_eq!(instance.render().expect("render"), vec![]);
        assert_eq!(instance.memory_bytes(), 16 << 20);

        let beyond = host
            .compile(spinning_component(257).as_bytes())
            .expect("component compiles");
        assert!(
            matches!(
                host.instantiate(&beyond),
                Err(TrialError::MemoryCap { cap_bytes }) if cap_bytes == 16 << 20
            ),
            "{backend:?}"
        );
    }
}

#[test]
fn a_call_that_runs_past_its_deadline_stops() {
    let limits = Limits {
        call_deadline: Duration::from_millis(100),
        ..Limits::default()
    };

    for backend in support::backends() {
        let host = TrialHost::with_limits(backend, limits).expect("host starts");
        let compiled = host
            .compile(spinning_component(1).as_bytes())
            .expect("component compiles");
        let mut instance = host.instantiate(&compiled).expect("component instantiates");

        let started = Instant::now();
        let result = instance.handle_event("anything");

        assert!(
            matches!(result, Err(TrialError::Deadline { deadline }) if deadline == limits.call_deadline),
            "{backend:?}: {result:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(5), "{backend:?}");
    }
}

#[test]
fn a_plugin_that_imports_wasi_files_sockets_or_http_does_not_load() {
    let host = TrialHost::new(Backend::Cranelift).expect("host starts");

    for import in [
        "wasi:filesystem/preopens@0.2.12",
        "wasi:sockets/tcp@0.2.12",
        "wasi:http/outgoing-handler@0.2.12",
    ] {
        let component =
            format!(r#"(component (import "{import}" (instance (export "f" (func)))))"#);

        let result = host.compile(component.as_bytes());

        assert!(
            matches!(&result, Err(TrialError::ForbiddenImport { import: named }) if named == import),
            "{import}: {:?}",
            result.err()
        );
    }
}
