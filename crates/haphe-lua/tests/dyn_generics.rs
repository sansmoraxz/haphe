//! Dyn-dispatched generic functions: one Lua callable scans the registered
//! monomorph candidates at call time.

#![allow(dead_code)]

use haphe::script;
#[cfg(feature = "generics")]
use haphe_lua::LuaBinder;
use mlua::Lua;

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

/// Static generic: stays rejected at bind time.
#[script(instantiate(i64))]
fn stat<T>(value: T) -> T {
    value
}

haphe::registry! {
    pub static REGISTRY = {
        modules: [
            mod m {
                functions: [which, floaty],
            },
        ],
    };
}

#[cfg(feature = "generics")]
fn bound_lua() -> Lua {
    use haphe::RuntimeBinder;
    let mut lua = Lua::new();
    let binder = LuaBinder::new();
    let validated = REGISTRY.validate().expect("registry validates");
    binder.bind(&validated, &mut lua).expect("binding succeeds");
    let table: mlua::Table = lua.load("return m").eval().expect("module table");
    haphe_lua::bind_fn::<which>(&lua, &table).expect("which binds");
    haphe_lua::bind_fn::<floaty>(&lua, &table).expect("floaty binds");
    lua
}

#[cfg(feature = "generics")]
#[test]
fn dyn_picks_candidate_by_argument_type() {
    let lua = bound_lua();
    let out: String = lua.load("return m.which(5)").eval().unwrap();
    assert_eq!(out, "i64");
    let out: String = lua.load("return m.which('hi')").eval().unwrap();
    assert_eq!(out, "alloc::string::String");
}

#[cfg(feature = "generics")]
#[test]
fn coercible_pass_picks_first_declared_candidate() {
    let lua = bound_lua();
    // An integer is Coercible to both float candidates; declaration order
    // breaks the tie: f64 first.
    let out: String = lua.load("return m.floaty(5)").eval().unwrap();
    assert_eq!(out, "f64");
    // A float is Exact for both; still first-declared.
    let out: String = lua.load("return m.floaty(1.5)").eval().unwrap();
    assert_eq!(out, "f64");
}

#[cfg(feature = "generics")]
#[test]
fn no_match_try_calls_in_order_then_lists_candidates() {
    let lua = bound_lua();
    // A boolean matches no candidate shape (TryCallOrder); every wrapper's
    // conversion rejects it, and the final error names each candidate
    // signature.
    let err = lua
        .load("return m.which(true)")
        .eval::<String>()
        .unwrap_err()
        .to_string();
    assert!(err.contains("no dyn candidate of `which`"), "got: {err}");
    assert!(err.contains("which<i64>(value: i64)"), "got: {err}");
    assert!(err.contains("which<string>(value: string)"), "got: {err}");
}

#[cfg(feature = "generics")]
#[test]
fn static_generic_binds_mangled_monomorphs() {
    // Static generics now monomorphize under mangled names (one table
    // entry per instantiation) instead of being rejected.
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    haphe_lua::bind_fn::<stat>(&lua, &table).unwrap();
    lua.globals().set("fns", table).unwrap();
    let out: i64 = lua.load("return fns.stat__i64(7)").eval().unwrap();
    assert_eq!(out, 7);
    // The unmangled name stays absent — static dispatch has no scan.
    let is_nil: bool = lua.load("return fns.stat == nil").eval().unwrap();
    assert!(is_nil);
}

#[cfg(not(feature = "generics"))]
#[test]
fn static_generic_still_rejected_at_bind() {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    let err = haphe_lua::bind_fn::<stat>(&lua, &table).unwrap_err();
    assert!(
        matches!(err, haphe_lua::LuaBindError::GenericFunction { name } if name == "stat"),
        "got: {err}"
    );
}

#[cfg(not(feature = "generics"))]
#[test]
fn dyn_rejected_without_generics_feature() {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    let err = haphe_lua::bind_fn::<which>(&lua, &table).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("dyn function `which`"), "got: {msg}");
    assert!(msg.contains("`generics` feature"), "got: {msg}");
}

#[test]
fn dyn_capability_gates_via_check() {
    use haphe::{BackendCapabilities, CompatibilityError};
    let validated = REGISTRY.validate().unwrap();
    // The dyn-capable configuration passes.
    BackendCapabilities::ALL.check(&validated).unwrap();
    // A backend without dyn machinery rejects descriptively.
    let errors = BackendCapabilities::ALL
        .with_dyn_generics(false)
        .check(&validated)
        .unwrap_err();
    assert!(
        errors.iter().any(|e| matches!(
            e,
            CompatibilityError::DynGenericsUnsupported { function: "which" }
        )),
        "{errors:?}"
    );
}

#[cfg(feature = "generics")]
#[test]
fn decl_stub_emits_overloads() {
    use haphe_lua::LuaDeclGenerator;
    let output = haphe::generate(&LuaDeclGenerator::new(), &REGISTRY).expect("generation succeeds");
    let out = String::from_utf8(output.files[0].content.clone()).unwrap();
    assert!(
        out.contains("---@overload fun(value: string): string"),
        "got:\n{out}"
    );
    assert!(out.contains("---@param value integer"), "got:\n{out}");
    assert!(out.contains("function m.which(value) end"), "got:\n{out}");
}

/// Async dyn generic: the same scan drives async candidates.
#[script(dyn, instantiate(i64), instantiate(String))]
async fn which_async<T>(value: T) -> String {
    #[cfg(feature = "async")]
    tokio::task::yield_now().await;
    let _ = value;
    std::any::type_name::<T>().to_string()
}

#[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
mod with_async {
    use super::*;

    #[tokio::test]
    async fn async_dyn_picks_candidate_by_argument_type() {
        let lua = Lua::new();
        let tbl = lua.create_table().unwrap();
        haphe_lua::bind_fn::<which_async>(&lua, &tbl).unwrap();
        lua.globals().set("m", &tbl).unwrap();
        let out: String = lua
            .load("return m.which_async(5)")
            .eval_async()
            .await
            .unwrap();
        assert_eq!(out, "i64");
        let out: String = lua
            .load("return m.which_async('hi')")
            .eval_async()
            .await
            .unwrap();
        assert_eq!(out, "alloc::string::String");
    }
}

/// Bare `dyn`: single-parameter generics auto-instantiate the default
/// candidate set {i64, f64, bool, String, char}, in that priority order.
#[script(dyn)]
fn mirror<T>(value: T) -> T {
    value
}

/// Candidate-identity twin of `mirror` for asserting which default
/// instantiation handled the call.
#[script(dyn)]
fn mirror_kind<T>(value: T) -> String {
    let _ = value;
    std::any::type_name::<T>().to_string()
}

/// Container candidates: ranked-then-reject fall-through.
#[script(dyn, instantiate(Vec<i64>), instantiate(Vec<String>))]
fn list_kind<T>(value: T) -> String {
    let _ = value;
    std::any::type_name::<T>().to_string()
}

#[cfg(feature = "generics")]
mod bare_dyn_and_containers {
    use super::*;

    fn env(lua: &Lua) {
        let tbl = lua.create_table().unwrap();
        haphe_lua::bind_fn::<mirror>(lua, &tbl).unwrap();
        haphe_lua::bind_fn::<mirror_kind>(lua, &tbl).unwrap();
        haphe_lua::bind_fn::<list_kind>(lua, &tbl).unwrap();
        lua.globals().set("m", &tbl).unwrap();
    }

    #[test]
    fn bare_dyn_default_candidates_round_trip() {
        let lua = Lua::new();
        env(&lua);
        let out: i64 = lua.load("return m.mirror(5)").eval().unwrap();
        assert_eq!(out, 5);
        let out: f64 = lua.load("return m.mirror(1.5)").eval().unwrap();
        assert_eq!(out, 1.5);
        let out: bool = lua.load("return m.mirror(true)").eval().unwrap();
        assert!(out);
        let out: String = lua.load("return m.mirror('hello')").eval().unwrap();
        assert_eq!(out, "hello");
        let out: String = lua.load("return m.mirror('x')").eval().unwrap();
        assert_eq!(out, "x");
    }

    #[test]
    fn bare_dyn_priority_order_decides_candidate() {
        let lua = Lua::new();
        env(&lua);
        let out: String = lua.load("return m.mirror_kind(5)").eval().unwrap();
        assert_eq!(out, "i64");
        let out: String = lua.load("return m.mirror_kind(1.5)").eval().unwrap();
        assert_eq!(out, "f64");
        let out: String = lua.load("return m.mirror_kind(true)").eval().unwrap();
        assert_eq!(out, "bool");
        // A single-char Lua string ranks Exact on the String candidate,
        // which PRECEDES char in the default priority order — char is
        // reachable only by fall-through, never by ranking. Documented
        // behavior of the default set.
        let out: String = lua.load("return m.mirror_kind('x')").eval().unwrap();
        assert_eq!(out, "alloc::string::String");
    }

    #[test]
    fn homogeneous_list_ranks_matching_container() {
        let lua = Lua::new();
        env(&lua);
        let out: String = lua.load("return m.list_kind({1, 2})").eval().unwrap();
        assert_eq!(out, "alloc::vec::Vec<i64>");
        let out: String = lua.load("return m.list_kind({'a', 'b'})").eval().unwrap();
        assert_eq!(out, "alloc::vec::Vec<alloc::string::String>");
    }

    #[test]
    fn heterogeneous_list_falls_through_after_ranked_reject() {
        let lua = Lua::new();
        env(&lua);
        // {1, 'x'}: the first-element heuristic ranks Vec<i64> Exact, but
        // FromScript rejects the string element — the scan falls through to
        // Vec<String>, whose conversion rejects the integer element too, so
        // the descriptive no-match error surfaces listing both candidates.
        let err = lua
            .load("return m.list_kind({1, 'x'})")
            .eval::<String>()
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("no dyn candidate of `list_kind`"),
            "got: {err}"
        );
        assert!(err.contains("list_kind<i64[]>"), "got: {err}");
        assert!(err.contains("list_kind<string[]>"), "got: {err}");
    }
}
