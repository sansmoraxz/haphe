//! Fallible surfaces through the VM: `Result<T, E>` constructors, methods,
//! and free functions bind; `Ok` crosses as the value, `Err` raises a Rhai
//! runtime error carrying the rendered callee message, prefixed by the
//! declared `error_kind`. Conversion errors keep their existing text.

#![allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    clippy::doc_markdown,
    clippy::trivially_copy_pass_by_ref,
    reason = "fixture shapes are dictated by the bridge surface under test: `#[script]` functions receive OWNED values (the boundary contract), methods keep their declared receivers (`&self` on Copy enums included), and docs name fixture idents verbatim"
)]
#![allow(
    dead_code,
    reason = "fixtures are exercised through their generated descriptors and bridge wrappers, not direct calls"
)]

use haphe::{Script, script};
use haphe_rhai::{bind_fn, bind_type};

#[derive(Script, Clone, Debug)]
#[script(thread_safety = send_sync, methods)]
struct Meter {
    level: i64,
}

#[script]
impl Meter {
    #[script(constructor)]
    fn new(level: i64) -> Result<Self, TextError> {
        if level < 0 {
            Err(TextError(format!("negative level {level}")))
        } else {
            Ok(Meter { level })
        }
    }

    #[script(error_kind = "RangeError")]
    fn checked_add(&self, amount: i64) -> Result<i64, TextError> {
        self.level
            .checked_add(amount)
            .ok_or_else(|| TextError("overflow".into()))
    }

    fn drain(&mut self, amount: i64) -> Result<(), TextError> {
        if amount > self.level {
            return Err(TextError("insufficient".into()));
        }
        self.level -= amount;
        Ok(())
    }

    fn remaining(&self) -> i64 {
        self.level
    }
}

fn env() -> rhai::Engine {
    let mut engine = rhai::Engine::new();
    let mut type_mod = rhai::Module::new();
    bind_type::<Meter>(&mut engine, &mut type_mod).expect("Meter binds");
    engine
}

#[test]
fn fallible_constructor_ok_and_err() {
    let engine = env();

    // Ok path: construct from Rhai script.
    let m: Meter = engine.eval("Meter(4)").expect("eval");
    assert_eq!(m.level, 4);

    // Err path: the constructor raises a Rhai error.
    let err = engine.eval::<Meter>("Meter(-1)").expect_err("should error");
    let msg = err.to_string();
    assert!(msg.contains("negative level -1"), "got: {msg}");
}

#[test]
fn fallible_method_ok_and_err() {
    let engine = env();

    let mut scope = rhai::Scope::new();
    scope.push("m", Meter { level: 40 });
    let sum: i64 = engine
        .eval_with_scope(&mut scope, "m.checked_add(2)")
        .expect("eval");
    assert_eq!(sum, 42);

    let mut scope2 = rhai::Scope::new();
    scope2.push("m", Meter { level: 1 });
    let err = engine
        .eval_with_scope::<i64>(&mut scope2, "m.checked_add(9223372036854775807)")
        .expect_err("overflow");
    let msg = err.to_string();
    assert!(msg.contains("RangeError"), "got: {msg}");
    assert!(msg.contains("overflow"), "got: {msg}");
}

#[test]
fn fallible_unit_method_mutates_or_raises() {
    let engine = env();

    let mut scope = rhai::Scope::new();
    scope.push("m", Meter { level: 10 });
    engine
        .run_with_scope(&mut scope, "m.drain(4);")
        .expect("drain succeeds");
    let left: i64 = engine
        .eval_with_scope(&mut scope, "m.remaining()")
        .expect("eval");
    assert_eq!(left, 6, "Ok(()) crosses as unit and mutation persists");

    let mut scope2 = rhai::Scope::new();
    scope2.push("m", Meter { level: 1 });
    let err = engine
        .eval_with_scope::<rhai::Dynamic>(&mut scope2, "m.drain(100)")
        .expect_err("insufficient");
    let msg = err.to_string();
    assert!(msg.contains("insufficient"), "got: {msg}");
}

#[script(error_kind = "ParseError")]
fn parse_level(raw: String) -> Result<i64, std::num::ParseIntError> {
    raw.parse()
}

#[test]
fn fallible_free_fn_ok_and_err() {
    let mut engine = env();
    bind_fn::<parse_level>(&mut engine).expect("binds");
    let result: i64 = engine.eval(r#"parse_level("42")"#).expect("eval");
    assert_eq!(result, 42);
    let err = engine
        .eval::<i64>(r#"parse_level("nope")"#)
        .expect_err("parse error");
    let msg = err.to_string();
    assert!(msg.contains("ParseError"), "got: {msg}");
}

/// A message-only fixture error.
#[derive(Debug)]
struct TextError(String);

impl std::fmt::Display for TextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TextError {}
