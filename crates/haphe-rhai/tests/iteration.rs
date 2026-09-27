//! Iteration and length: `traits(IntoIterator)` / `traits(Iterator)` drive
//! `to_array()` and `len()` in the Rhai backend.

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

use haphe::Script;
use haphe_rhai::bind_type;

#[derive(Script, Clone)]
#[script(thread_safety = send_sync, traits(IntoIterator(item = i64)))]
struct Sequence {
    #[script(skip)]
    values: Vec<i64>,
}

impl IntoIterator for Sequence {
    type Item = i64;
    type IntoIter = std::vec::IntoIter<i64>;
    fn into_iter(self) -> Self::IntoIter {
        self.values.into_iter()
    }
}

#[derive(Script, Clone)]
#[script(thread_safety = send_sync, traits(Iterator(item = i64)))]
struct Countdown {
    #[script(skip)]
    remaining: i64,
}

impl Iterator for Countdown {
    type Item = i64;
    fn next(&mut self) -> Option<i64> {
        (self.remaining > 0).then(|| {
            self.remaining -= 1;
            self.remaining + 1
        })
    }
}

fn seq_engine() -> rhai::Engine {
    let mut engine = rhai::Engine::new();
    let mut m = rhai::Module::new();
    bind_type::<Sequence>(&mut engine, &mut m).expect("binds Sequence");
    engine
}

fn countdown_engine() -> rhai::Engine {
    let mut engine = rhai::Engine::new();
    let mut m = rhai::Module::new();
    bind_type::<Countdown>(&mut engine, &mut m).expect("binds Countdown");
    engine
}

#[test]
fn to_array_yields_all_items() {
    let engine = seq_engine();
    let mut scope = rhai::Scope::new();
    scope.push("s", Sequence { values: vec![10, 20, 30] });
    let arr: rhai::Array = engine
        .eval_with_scope(&mut scope, "s.to_array()")
        .expect("eval");
    let vals: Vec<i64> = arr.into_iter().map(|d| d.as_int().unwrap()).collect();
    assert_eq!(vals, vec![10, 20, 30]);
}

#[test]
fn len_reports_size() {
    let engine = seq_engine();
    let mut scope = rhai::Scope::new();
    scope.push("s", Sequence { values: vec![10, 20, 30] });
    let len: i64 = engine
        .eval_with_scope(&mut scope, "s.len()")
        .expect("eval");
    assert_eq!(len, 3);
}

#[test]
fn empty_collection_yields_empty_array() {
    let engine = seq_engine();
    let mut scope = rhai::Scope::new();
    scope.push("s", Sequence { values: vec![] });
    let arr: rhai::Array = engine
        .eval_with_scope(&mut scope, "s.to_array()")
        .expect("eval");
    assert!(arr.is_empty());
    let len: i64 = engine
        .eval_with_scope(&mut scope, "s.len()")
        .expect("eval");
    assert_eq!(len, 0);
}

#[test]
fn iterator_trait_bridges_like_into_iterator() {
    let engine = countdown_engine();
    let mut scope = rhai::Scope::new();
    scope.push("c", Countdown { remaining: 3 });
    let arr: rhai::Array = engine
        .eval_with_scope(&mut scope, "c.to_array()")
        .expect("eval");
    let vals: Vec<i64> = arr.into_iter().map(|d| d.as_int().unwrap()).collect();
    assert_eq!(vals, vec![3, 2, 1]);
}
