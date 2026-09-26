//! Computed properties (`#[script(getter)]`/`#[script(setter)]`) bound as
//! Rhai property accessors.

#![allow(
    dead_code,
    reason = "fixtures are exercised through their generated descriptors and bridge wrappers, not direct calls"
)]

use haphe::{Script, script};
use haphe_rhai::bind_type;

#[derive(Script, Clone)]
#[script(thread_safety = send_sync, methods)]
struct Gauge {
    #[script(skip)]
    raw: i64,
}

#[script]
impl Gauge {
    #[script(constructor)]
    fn new(raw: i64) -> Self {
        Gauge { raw }
    }

    #[script(getter)]
    fn level(&self) -> i64 {
        self.raw * 2
    }

    #[script(setter)]
    fn set_level(&mut self, value: i64) {
        self.raw = value / 2;
    }

    #[script(getter)]
    fn raw_value(&self) -> i64 {
        self.raw
    }
}

fn env() -> rhai::Engine {
    let mut engine = rhai::Engine::new();
    let mut type_mod = rhai::Module::new();
    bind_type::<Gauge>(&mut engine, &mut type_mod).expect("Gauge binds");
    engine
}

#[test]
fn property_reads_and_writes_as_field() {
    let engine = env();

    let mut scope = rhai::Scope::new();
    scope.push("g", Gauge::new(21));
    let out: i64 = engine
        .eval_with_scope(&mut scope, "g.level")
        .expect("eval");
    assert_eq!(out, 42);

    let mut scope2 = rhai::Scope::new();
    scope2.push("g", Gauge::new(0));
    engine
        .run_with_scope(&mut scope2, "g.level = 10;")
        .expect("set");
    let out: i64 = engine
        .eval_with_scope(&mut scope2, "g.raw_value")
        .expect("eval");
    assert_eq!(out, 5);
}

#[test]
fn readonly_property_has_no_setter() {
    let engine = env();
    let mut scope = rhai::Scope::new();
    scope.push("g", Gauge::new(1));
    let err = engine
        .run_with_scope(&mut scope, "g.raw_value = 9;")
        .expect_err("no setter registered");
    let msg = err.to_string();
    assert!(!msg.is_empty(), "got: {msg}");
}

#[test]
fn property_set_conversion_failure_errors() {
    let engine = env();
    let mut scope = rhai::Scope::new();
    scope.push("g", Gauge::new(1));
    let err = engine
        .run_with_scope(&mut scope, r#"g.level = "nope";"#)
        .expect_err("string does not convert to i64");
    let msg = err.to_string();
    assert!(!msg.is_empty(), "got: {msg}");
}
