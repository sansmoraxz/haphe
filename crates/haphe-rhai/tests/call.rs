//! Callable userdata: `traits(Call)` drives the `call` method registration.

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
use haphe_rhai::bind_type;

#[derive(Script, Clone)]
#[script(thread_safety = send_sync, traits(Call(args = (i64, i64), output = i64)), methods)]
struct Adder {
    base: i64,
}

impl haphe::ops::Call<(i64, i64)> for Adder {
    type Output = i64;
    fn call(&self, args: (i64, i64)) -> i64 {
        self.base + args.0 + args.1
    }
}

#[script]
impl Adder {
    #[script(constructor)]
    fn new(base: i64) -> Self {
        Adder { base }
    }
}

#[derive(Script, Clone)]
#[script(thread_safety = send_sync, traits(Call(args = (), output = String)), methods)]
struct Ping {
    word: String,
}

impl haphe::ops::Call<()> for Ping {
    type Output = String;
    fn call(&self, _args: ()) -> String {
        format!("{}!", self.word)
    }
}

#[script]
impl Ping {
    #[script(constructor)]
    fn new(word: String) -> Self {
        Ping { word }
    }
}

fn adder_engine() -> rhai::Engine {
    let mut engine = rhai::Engine::new();
    let mut m = rhai::Module::new();
    bind_type::<Adder>(&mut engine, &mut m).expect("binds");
    engine
}

fn ping_engine() -> rhai::Engine {
    let mut engine = rhai::Engine::new();
    let mut m = rhai::Module::new();
    bind_type::<Ping>(&mut engine, &mut m).expect("binds");
    engine
}

#[test]
fn sync_call_works() {
    let engine = adder_engine();
    let mut scope = rhai::Scope::new();
    scope.push("a", Adder { base: 100 });
    let result: i64 = engine
        .eval_with_scope(&mut scope, "a.invoke(2, 3)")
        .expect("eval");
    assert_eq!(result, 105);
}

#[test]
fn zero_arg_call_works() {
    let engine = ping_engine();
    let mut scope = rhai::Scope::new();
    scope.push("p", Ping { word: "pong".to_string() });
    let result: String = engine
        .eval_with_scope(&mut scope, "p.invoke()")
        .expect("eval");
    assert_eq!(result, "pong!");
}

#[test]
fn wrong_arg_type_errors() {
    let engine = adder_engine();
    let mut scope = rhai::Scope::new();
    scope.push("a", Adder { base: 100 });
    let err = engine
        .eval_with_scope::<i64>(&mut scope, "a.invoke(\"two\", 3)")
        .unwrap_err();
    assert!(!err.to_string().is_empty());
}
