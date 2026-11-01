//! A plugin built against one interface version keeps working when Noura
//! serves a newer one (plugin-host's manifest lists the versions it runs).
//! Noura has one version today, so a stand-in plays the later host: it
//! names every interface at 0.1.1, as a release that adds to the interface
//! would, and runs the real hello plugin, built against 0.1.0.

mod support;

use plugin_host::trap_foreign_imports;
use support::plugins::built;
use wasmtime::component::{Component, Linker, Val};
use wasmtime::{Config, Engine, Store};

const LATER: &str = "0.1.1";

#[test]
fn a_plugin_built_against_0_1_0_runs_in_a_host_that_serves_a_later_version() {
    let Some(hello) = built("hello_plugin") else {
        return;
    };
    let mut config = Config::new();
    config.wasm_component_model(true);
    let engine = Engine::new(&config).expect("engine");
    let component = Component::new(&engine, hello).expect("the plugin compiles");

    // The later host answers the plugin's import under its own version.
    let mut linker = Linker::<u32>::new(&engine);
    linker
        .instance(&format!("noura:plugin/workspace@{LATER}"))
        .expect("the interface")
        .func_new("list-entries", |mut store, _type, _arguments, results| {
            *store.data_mut() += 1;
            results[0] = Val::Result(Ok(Some(Box::new(Val::List(Vec::new())))));
            Ok(())
        })
        .expect("the function");
    trap_foreign_imports(&mut linker, &component).expect("the other imports");
    let mut store = Store::new(&engine, 0);
    let instance = linker
        .instantiate(&mut store, &component)
        .expect("the older plugin links");

    // And finds the plugin's export the way the host does, by its own name.
    let view = component
        .get_export_index(None, &format!("noura:plugin/view@{LATER}"))
        .expect("the older export answers to the later name");
    let render = component
        .get_export_index(Some(&view), "render")
        .expect("the view's render");
    let render = instance.get_func(&mut store, render).expect("a function");
    let mut nodes = [Val::Bool(false)];
    render
        .call(&mut store, &[Val::String("hello".into())], &mut nodes)
        .expect("the older plugin draws");

    assert_eq!(*store.data(), 1, "the plugin called the later host");
    let drawn = format!("{:?}", nodes[0]);
    assert!(drawn.contains("This workspace has 0 files."), "{drawn}");
}
