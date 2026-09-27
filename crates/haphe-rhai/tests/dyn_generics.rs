//! Dyn-dispatched generic free functions: one Rhai callable scans the
//! registered monomorph candidates at call time.

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

use haphe::script;

/// Names the instantiation that handled the call.
#[script(dyn, instantiate(i64), instantiate(String))]
fn which<T>(value: T) -> String {
    let _ = value;
    std::any::type_name::<T>().to_string()
}

/// Coercible-pass fixture: only float candidates.
#[script(dyn, instantiate(f64), instantiate(f32))]
fn floaty<T>(value: T) -> String {
    let _ = value;
    std::any::type_name::<T>().to_string()
}

/// Static generic: always binds under its mangled name.
#[script(instantiate(i64))]
fn stat<T>(value: T) -> T {
    value
}

#[cfg(feature = "generics")]
#[test]
fn dyn_free_fn_binds_without_error() {
    let mut engine = rhai::Engine::new();
    haphe_rhai::bind_fn::<which>(&mut engine).expect("dyn free fn binds");
    haphe_rhai::bind_fn::<floaty>(&mut engine).expect("dyn free fn binds");
}

#[cfg(feature = "generics")]
#[test]
fn static_generic_free_fn_binds() {
    let mut engine = rhai::Engine::new();
    haphe_rhai::bind_fn::<stat>(&mut engine).expect("static generic free fn binds");
}

#[cfg(not(feature = "generics"))]
#[test]
fn static_generic_free_fn_binds_without_error() {
    let mut engine = rhai::Engine::new();
    haphe_rhai::bind_fn::<stat>(&mut engine)
        .expect("static generic free fns bind even without `generics`");
}

#[cfg(not(feature = "generics"))]
#[test]
fn dyn_rejected_without_generics_feature() {
    let mut engine = rhai::Engine::new();
    let err = haphe_rhai::bind_fn::<which>(&mut engine).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("dyn function `which`"), "got: {msg}");
    assert!(msg.contains("`generics` feature"), "got: {msg}");
}
