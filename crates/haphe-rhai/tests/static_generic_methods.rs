//! Statically dispatched generic methods and free functions: one Rhai
//! callable per declared instantiation under its mangled monomorph name
//! (`first_of__i64`) — exact dispatch, no scan, no fall-through.

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
struct Pair {
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
impl Pair {
    #[script(constructor)]
    fn new(total: i64) -> Self {
        Pair { total }
    }

    #[script(instantiate(i64), instantiate(String))]
    fn first_of<T>(&self, a: T, b: T) -> T {
        let _ = b;
        a
    }

    #[script(instantiate(i64), instantiate(f64))]
    fn add_in<T: Acc>(&mut self, value: T) {
        self.total += value.acc();
    }

    fn get(&self) -> i64 {
        self.total
    }
}

#[script(instantiate(i64), instantiate(String))]
fn echo<T>(value: T) -> T {
    value
}

haphe::registry! {
    pub static REGISTRY = {
        structs: [Pair],
        modules: [
            mod m {
                types: [Pair],
                functions: [echo],
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
    haphe_rhai::bind_type::<Pair>(&mut engine, &mut type_mod).expect("Pair binds");
    engine
}

#[cfg(feature = "generics")]
#[test]
fn monomorphs_dispatch_by_mangled_name() {
    let engine = bound_engine();
    let mut scope = rhai::Scope::new();
    scope.push("p", Pair::new(0));
    let out: i64 = engine
        .eval_with_scope(&mut scope, "p.first_of__i64(4, 9)")
        .unwrap();
    assert_eq!(out, 4);
    let out: String = engine
        .eval_with_scope(&mut scope, r#"p.first_of__string("a", "b")"#)
        .unwrap();
    assert_eq!(out, "a");
}

#[cfg(feature = "generics")]
#[test]
fn unmangled_name_is_not_callable() {
    let engine = bound_engine();
    let mut scope = rhai::Scope::new();
    scope.push("p", Pair::new(0));
    let err = engine
        .eval_with_scope::<i64>(&mut scope, "p.first_of(4, 9)")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("first_of") || err.contains("not found"),
        "got: {err}"
    );
}

#[cfg(feature = "generics")]
#[test]
fn mut_monomorphs_write_back() {
    let engine = bound_engine();
    let mut scope = rhai::Scope::new();
    scope.push("p", Pair::new(1));
    engine
        .run_with_scope(&mut scope, "p.add_in__i64(5); p.add_in__f64(2.0);")
        .unwrap();
    let out: i64 = engine.eval_with_scope(&mut scope, "p.get()").unwrap();
    assert_eq!(out, 8);
}

#[cfg(feature = "generics")]
#[test]
fn wrong_typed_argument_errors_without_fallthrough() {
    let engine = bound_engine();
    let mut scope = rhai::Scope::new();
    scope.push("p", Pair::new(0));
    let err = engine
        .eval_with_scope::<i64>(&mut scope, r#"p.first_of__i64("a", "b")"#)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("i64") || err.contains("expected"),
        "got: {err}"
    );
}

#[cfg(feature = "generics")]
#[test]
fn free_fn_monomorphs_bind() {
    let mut engine = bound_engine();
    haphe_rhai::bind_fn::<echo>(&mut engine).unwrap();
    let out: i64 = engine.eval("echo__i64(7)").unwrap();
    assert_eq!(out, 7);
    let out: String = engine.eval(r#"echo__string("hi")"#).unwrap();
    assert_eq!(out, "hi");
}

#[cfg(not(feature = "generics"))]
#[test]
fn static_generic_methods_rejected_without_feature() {
    let mut engine = rhai::Engine::new();
    let mut type_mod = rhai::Module::new();
    let err = haphe_rhai::bind_type::<Pair>(&mut engine, &mut type_mod).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("first_of") || msg.contains("add_in"),
        "got: {msg}"
    );
    assert!(msg.contains("generic"), "got: {msg}");
}

#[cfg(feature = "generics")]
#[test]
fn decl_stub_names_each_monomorph() {
    let output =
        haphe::generate(&haphe_rhai::RhaiDeclGenerator::new(), &REGISTRY).expect("generates");
    let content = std::str::from_utf8(&output.files[0].content).expect("utf-8");
    assert!(content.contains("first_of__i64"), "got:\n{content}");
    assert!(content.contains("first_of__string"), "got:\n{content}");
    assert!(content.contains("add_in__f64"), "got:\n{content}");
    assert!(content.contains("echo__i64"), "got:\n{content}");
    assert!(content.contains("echo__string"), "got:\n{content}");
}
