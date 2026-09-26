//! `TypeDescriptor::Borrowed` (lifetime carriers like `Cow`): Rhai has no
//! borrow semantics.  With the `borrowed` feature, values cross as the inner
//! type (cloned at the boundary).  Without it, binding is rejected.

#![allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    clippy::doc_markdown,
    reason = "fixture shapes are dictated by the bridge surface under test: `#[script]` functions receive OWNED values (the boundary contract), methods keep their declared receivers (`&self` on Copy enums included), and docs name fixture idents verbatim"
)]
#![allow(
    dead_code,
    reason = "fixtures are exercised through their generated descriptors and bridge wrappers, not direct calls"
)]

use std::borrow::Cow;

use haphe::{Script, script};
use haphe_rhai::bind_type;

#[derive(Script, Clone)]
#[script(thread_safety = send_sync, methods)]
struct Tag {
    name: String,
}

#[script]
impl Tag {
    #[script(constructor)]
    fn new(name: String) -> Self {
        Tag { name }
    }

    fn suffixed(&self, base: Cow<'_, str>) -> String {
        format!("{base}-{}", self.name)
    }

    fn label(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.name)
    }

    fn shout(&self, text: Cow<'_, str>) -> String {
        text.to_uppercase()
    }
}

// ---- with `borrowed` feature: values cross as copies ----

#[cfg(feature = "borrowed")]
fn bound_engine() -> rhai::Engine {
    let mut engine = rhai::Engine::new();
    let mut type_mod = rhai::Module::new();
    bind_type::<Tag>(&mut engine, &mut type_mod).expect("binds Tag");
    engine
}

#[cfg(feature = "borrowed")]
#[test]
fn cow_method_param_and_return_round_trip() {
    let engine = bound_engine();
    let mut scope = rhai::Scope::new();
    scope.push("t", Tag::new("x".to_string()));

    let suffixed: String = engine
        .eval_with_scope(&mut scope, r#"t.suffixed("base")"#)
        .expect("eval");
    assert_eq!(suffixed, "base-x");

    let label: String = engine
        .eval_with_scope(&mut scope, "t.label()")
        .expect("eval");
    assert_eq!(label, "x");
}

#[cfg(feature = "borrowed")]
#[test]
fn cow_method_upper_crosses_as_string() {
    let engine = bound_engine();
    let mut scope = rhai::Scope::new();
    scope.push("t", Tag::new("any".to_string()));
    let out: String = engine
        .eval_with_scope(&mut scope, r#"t.shout("hey")"#)
        .expect("eval");
    assert_eq!(out, "HEY");
}

// ---- without `borrowed` feature: binding is rejected ----

#[cfg(not(feature = "borrowed"))]
#[test]
fn borrowed_type_rejected_without_feature() {
    let mut engine = rhai::Engine::new();
    let mut type_mod = rhai::Module::new();
    let err = bind_type::<Tag>(&mut engine, &mut type_mod).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("borrowed"), "got: {msg}");
    assert!(msg.contains("Cow"), "got: {msg}");
}
