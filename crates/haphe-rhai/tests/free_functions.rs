//! Free function binding via `bind_fn`.

#![allow(
    clippy::needless_pass_by_value,
    clippy::doc_markdown,
    reason = "fixture shapes are dictated by the bridge surface under test: `#[script]` functions receive OWNED values (the boundary contract) and docs name fixture idents verbatim"
)]

use haphe::script;

#[script]
fn add(a: i64, b: i64) -> i64 {
    a + b
}

#[script]
fn greet(name: String) -> String {
    format!("hello, {name}!")
}

#[test]
fn free_fn_dispatches() {
    let mut engine = rhai::Engine::new();
    haphe_rhai::bind_fn::<add>(&mut engine).expect("binds");
    let result: i64 = engine.eval("add(2, 40)").expect("eval");
    assert_eq!(result, 42);
}

#[test]
fn free_fn_string_args() {
    let mut engine = rhai::Engine::new();
    haphe_rhai::bind_fn::<greet>(&mut engine).expect("binds");
    let result: String = engine.eval(r#"greet("ada")"#).expect("eval");
    assert_eq!(result, "hello, ada!");
}
