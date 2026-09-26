//! Indexed access: `traits(Index/IndexMut)` drive `register_indexer_get`
//! and `register_indexer_set` in the Rhai backend.

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
#[script(
    thread_safety = send_sync,
    traits(Index(index = i64, output = i64), IndexMut(index = i64, output = i64)),
    methods,
)]
struct Buffer {
    label: String,
    #[script(skip)]
    data: Vec<i64>,
}

#[haphe::script]
impl Buffer {
    fn total(&self) -> i64 {
        self.data.iter().sum()
    }
}

impl haphe::ops::Index<i64> for Buffer {
    type Output = i64;
    fn index(&self, i: i64) -> &i64 {
        &self.data[usize::try_from(i).expect("negative index")]
    }
}

impl haphe::ops::IndexMut<i64> for Buffer {
    fn index_mut(&mut self, i: i64) -> &mut i64 {
        &mut self.data[usize::try_from(i).expect("negative index")]
    }
}

fn buf_engine() -> rhai::Engine {
    let mut engine = rhai::Engine::new();
    let mut m = rhai::Module::new();
    bind_type::<Buffer>(&mut engine, &mut m).expect("binds Buffer");
    engine
}

fn test_buf() -> Buffer {
    Buffer {
        label: "mine".into(),
        data: vec![5, 8, 3],
    }
}

#[test]
fn index_reads_and_writes() {
    let engine = buf_engine();
    let mut scope = rhai::Scope::new();
    scope.push("b", test_buf());

    let val: i64 = engine
        .eval_with_scope(&mut scope, "b[1]")
        .expect("read");
    assert_eq!(val, 8);

    engine
        .run_with_scope(&mut scope, "b[0] = 99;")
        .expect("write");
    let val: i64 = engine
        .eval_with_scope(&mut scope, "b[0]")
        .expect("read back");
    assert_eq!(val, 99);
}

#[test]
fn fields_and_methods_coexist_with_indexing() {
    let engine = buf_engine();
    let mut scope = rhai::Scope::new();
    scope.push("b", test_buf());

    let label: String = engine
        .eval_with_scope(&mut scope, "b.label")
        .expect("field read");
    assert_eq!(label, "mine");

    let total: i64 = engine
        .eval_with_scope(&mut scope, "b.total()")
        .expect("method call");
    assert_eq!(total, 16);

    let elem: i64 = engine
        .eval_with_scope(&mut scope, "b[2]")
        .expect("index still works");
    assert_eq!(elem, 3);
}

#[test]
#[should_panic(expected = "index out of bounds")]
fn out_of_bounds_index_panics() {
    let engine = buf_engine();
    let mut scope = rhai::Scope::new();
    scope.push("b", test_buf());

    let _ = engine.eval_with_scope::<i64>(&mut scope, "b[10]");
}
