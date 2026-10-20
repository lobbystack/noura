//! Link check for trial S0: Wasmtime 48 compiles and runs a WebAssembly
//! component with Cranelift.

use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store};

const ANSWER_COMPONENT: &str = r#"
(component
  (core module $m
    (func (export "answer") (result i32) i32.const 42))
  (core instance $i (instantiate $m))
  (func (export "answer") (result u32)
    (canon lift (core func $i "answer"))))
"#;

#[test]
fn wasmtime_runs_a_component_export() {
    let mut config = Config::new();
    config.wasm_component_model(true);
    let engine = Engine::new(&config).expect("engine");
    let component = Component::new(&engine, ANSWER_COMPONENT).expect("component compiles");
    let mut store = Store::new(&engine, ());
    let instance = Linker::new(&engine)
        .instantiate(&mut store, &component)
        .expect("component instantiates");
    let answer = instance
        .get_typed_func::<(), (u32,)>(&mut store, "answer")
        .expect("typed export");

    assert_eq!(answer.call(&mut store, ()).expect("call"), (42,));
}
