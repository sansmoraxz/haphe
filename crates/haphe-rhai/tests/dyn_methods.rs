//! Dyn-dispatched generic methods: one Rhai method per name scans the
//! registered monomorph candidates at call time, on struct and enum
//! userdata alike.

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

#[derive(Script, Clone)]
#[script(thread_safety = send_sync, methods)]
struct Holder {
    total: i64,
}

trait Acc {
    fn acc(self) -> i64;
}

impl Acc for i64 {
    fn acc(self) -> i64 {
        self
    }
}

impl Acc for f64 {
    fn acc(self) -> i64 {
        self as i64
    }
}

#[script]
impl Holder {
    #[script(constructor)]
    fn new(total: i64) -> Self {
        Holder { total }
    }

    #[script(dyn)]
    fn mirror<T>(&self, value: T) -> T {
        value
    }

    #[script(dyn, instantiate(f64), instantiate(f32))]
    fn floaty<T>(&self, value: T) -> String {
        let _ = value;
        std::any::type_name::<T>().to_string()
    }

    #[script(dyn, instantiate(i64), instantiate(f64))]
    fn accumulate<T: Acc>(&mut self, value: T) {
        self.total += value.acc();
    }

    fn get(&self) -> i64 {
        self.total
    }
}

#[derive(Script, Clone, Copy)]
#[script(thread_safety = send_sync, methods)]
enum Mode {
    Fast,
    Slow,
}

#[script]
impl Mode {
    #[script(dyn, instantiate(i64), instantiate(bool))]
    fn pick<T>(&self, a: T, b: T) -> T {
        match self {
            Mode::Fast => a,
            Mode::Slow => b,
        }
    }
}

haphe::registry! {
    pub static REGISTRY = {
        structs: [Holder],
        enums: [Mode],
        modules: [
            mod m {
                types: [Holder, Mode],
            },
        ],
    };
}

#[cfg(feature = "generics")]
fn bound_engine() -> rhai::Engine {
    use haphe::RuntimeBinder;
    let mut engine = rhai::Engine::new();
    let binder = haphe_rhai::RhaiBinder::new();
    let validated = REGISTRY.validate().expect("registry validates");
    binder.bind(&validated, &mut engine).expect("binding succeeds");
    let mut type_mod = rhai::Module::new();
    haphe_rhai::bind_type::<Holder>(&mut engine, &mut type_mod).expect("Holder binds");
    let mut enum_mod = rhai::Module::new();
    haphe_rhai::bind_enum_type::<Mode>(&mut engine, &mut enum_mod).expect("Mode binds");
    engine
}

#[cfg(feature = "generics")]
#[test]
fn bare_dyn_method_round_trips_default_candidates() {
    let engine = bound_engine();
    let mut scope = rhai::Scope::new();
    scope.push("h", Holder::new(0));
    let out: i64 = engine.eval_with_scope(&mut scope, "h.mirror(5)").unwrap();
    assert_eq!(out, 5);
    let out: f64 = engine
        .eval_with_scope(&mut scope, "h.mirror(1.5)")
        .unwrap();
    assert_eq!(out, 1.5);
    let out: String = engine
        .eval_with_scope(&mut scope, r#"h.mirror("hello")"#)
        .unwrap();
    assert_eq!(out, "hello");
}

#[cfg(feature = "generics")]
#[test]
fn coercible_pass_ties_break_by_declaration_order() {
    let engine = bound_engine();
    let mut scope = rhai::Scope::new();
    scope.push("h", Holder::new(0));
    let out: String = engine
        .eval_with_scope(&mut scope, "h.floaty(1.5)")
        .unwrap();
    assert_eq!(out, "f64");
}

#[cfg(feature = "generics")]
#[test]
fn mut_dyn_method_writes_back() {
    let engine = bound_engine();
    let mut scope = rhai::Scope::new();
    scope.push("h", Holder::new(1));
    engine
        .run_with_scope(&mut scope, "h.accumulate(5); h.accumulate(2.0);")
        .unwrap();
    let out: i64 = engine.eval_with_scope(&mut scope, "h.get()").unwrap();
    assert_eq!(out, 8);
}

#[cfg(feature = "generics")]
#[test]
fn no_match_lists_candidate_signatures() {
    let engine = bound_engine();
    let mut scope = rhai::Scope::new();
    scope.push("h", Holder::new(0));
    let err = engine
        .eval_with_scope::<String>(&mut scope, "h.floaty(true)")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("no dyn candidate") && err.contains("floaty"),
        "got: {err}"
    );
}

#[cfg(feature = "generics")]
#[test]
fn enum_dyn_method_dispatches_on_userdata() {
    let engine = bound_engine();
    let mut scope = rhai::Scope::new();
    scope.push("fast", Mode::Fast);
    scope.push("slow", Mode::Slow);
    let out: i64 = engine
        .eval_with_scope(&mut scope, "fast.pick(1, 2)")
        .unwrap();
    assert_eq!(out, 1);
    let out: bool = engine
        .eval_with_scope(&mut scope, "slow.pick(true, false)")
        .unwrap();
    assert!(!out);
}

#[cfg(not(feature = "generics"))]
#[test]
fn dyn_methods_rejected_without_feature() {
    let mut engine = rhai::Engine::new();
    let mut type_mod = rhai::Module::new();
    let err = haphe_rhai::bind_type::<Holder>(&mut engine, &mut type_mod).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("mirror") || msg.contains("dyn"), "got: {msg}");
    assert!(msg.contains("generic"), "got: {msg}");
}
