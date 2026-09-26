//! Hash and Debug method registration: `traits(Hash)` registers `hash()`,
//! `traits(Debug)` registers `to_debug()`.

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

use std::hash::{Hash, Hasher};

use haphe::{Script, script};
use haphe_rhai::bind_type;

#[derive(Script, Clone, Debug, PartialEq, Eq)]
#[script(thread_safety = send_sync, traits(Hash, Debug, PartialEq), methods)]
struct Tag {
    id: i64,
    label: String,
}

impl Hash for Tag {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        self.label.hash(state);
    }
}

#[script]
impl Tag {
    #[script(constructor)]
    fn new(id: i64, label: String) -> Self {
        Tag { id, label }
    }
}

fn bound_engine() -> rhai::Engine {
    let mut engine = rhai::Engine::new();
    let mut type_mod = rhai::Module::new();
    bind_type::<Tag>(&mut engine, &mut type_mod).expect("binds Tag");
    engine
}

#[test]
fn hash_method_is_stable_and_equal_for_equal_values() {
    let engine = bound_engine();
    let mut scope = rhai::Scope::new();
    scope.push("a", Tag::new(1, "one".to_string()));
    scope.push("b", Tag::new(1, "one".to_string()));
    let ha: i64 = engine
        .eval_with_scope(&mut scope, "a.hash()")
        .expect("eval");
    let hb: i64 = engine
        .eval_with_scope(&mut scope, "b.hash()")
        .expect("eval");
    assert_eq!(ha, hb);
}

#[test]
fn debug_method_matches_rust_formatting() {
    let engine = bound_engine();
    let tag = Tag::new(42, "test".to_string());
    let expected = format!("{tag:?}");
    let mut scope = rhai::Scope::new();
    scope.push("t", tag);
    let actual: String = engine
        .eval_with_scope(&mut scope, "t.to_debug()")
        .expect("eval");
    assert_eq!(actual, expected);
}
