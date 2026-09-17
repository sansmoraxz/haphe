//! Statically dispatched generic methods and free functions: one Lua
//! callable per declared instantiation under its mangled monomorph name
//! (`first_of__i64`) — exact dispatch, no scan, no fall-through.

#![allow(dead_code)]

use haphe::{Script, script};
use mlua::Lua;

#[derive(Script, Clone)]
#[script(methods)]
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

    /// Two static monomorphs: `first_of__i64` and `first_of__string`.
    #[script(instantiate(i64), instantiate(String))]
    fn first_of<T>(&self, a: T, b: T) -> T {
        let _ = b;
        a
    }

    /// `&mut self` static monomorphs mutate the bound userdata in place.
    #[script(instantiate(i64), instantiate(f64))]
    fn add_in<T: Acc>(&mut self, value: T) {
        self.total += value.acc();
    }

    fn get(&self) -> i64 {
        self.total
    }
}

/// Static generic free functions monomorphize the same way, on the module
/// table.
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
fn bound_lua() -> Lua {
    use haphe::RuntimeBinder;
    let mut lua = Lua::new();
    let binder = haphe_lua::LuaBinder::new();
    let validated = REGISTRY.validate().expect("registry validates");
    binder.bind(&validated, &mut lua).expect("binding succeeds");
    let pair_table: mlua::Table = lua.load("return m.Pair").eval().expect("type table");
    haphe_lua::bind_type::<Pair>(&lua, &pair_table).expect("Pair binds");
    lua
}

#[cfg(feature = "generics")]
#[test]
fn monomorphs_dispatch_by_mangled_name() {
    let lua = bound_lua();
    let out: i64 = lua
        .load("return m.Pair.new(0):first_of__i64(4, 9)")
        .eval()
        .unwrap();
    assert_eq!(out, 4);
    let out: String = lua
        .load("return m.Pair.new(0):first_of__string('a', 'b')")
        .eval()
        .unwrap();
    assert_eq!(out, "a");
    // The unmangled name is NOT a callable — static dispatch has no scan.
    let err = lua
        .load("return m.Pair.new(0):first_of(4, 9)")
        .eval::<i64>()
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("first_of") || err.contains("nil"),
        "got: {err}"
    );
}

#[cfg(feature = "generics")]
#[test]
fn mut_monomorphs_write_back_to_the_userdata() {
    let lua = bound_lua();
    let out: i64 = lua
        .load(
            "local p = m.Pair.new(1)
             p:add_in__i64(5)
             p:add_in__f64(2.0)
             return p:get()",
        )
        .eval()
        .unwrap();
    assert_eq!(out, 8);
}

#[cfg(feature = "generics")]
#[test]
fn wrong_typed_argument_errors_without_fallthrough() {
    let lua = bound_lua();
    // Static dispatch is exact: a string into the i64 monomorph is a
    // conversion error, never a scan onto the string monomorph.
    let err = lua
        .load("return m.Pair.new(0):first_of__i64('a', 'b')")
        .eval::<i64>()
        .unwrap_err()
        .to_string();
    assert!(err.contains("expected i64"), "got: {err}");
}

#[cfg(feature = "generics")]
#[test]
fn free_fn_monomorphs_land_on_the_bound_table() {
    let lua = bound_lua();
    // Module tables carry decl-level stubs; runtime free-fn binding goes
    // through `bind_fn` (same as non-generic free functions).
    let fns = lua.create_table().unwrap();
    haphe_lua::bind_fn::<echo>(&lua, &fns).unwrap();
    lua.globals().set("fns", fns).unwrap();
    let out: i64 = lua.load("return fns.echo__i64(7)").eval().unwrap();
    assert_eq!(out, 7);
    let out: String = lua.load("return fns.echo__string('hi')").eval().unwrap();
    assert_eq!(out, "hi");
}

#[cfg(feature = "generics")]
#[test]
fn decl_stub_names_each_monomorph() {
    use haphe_lua::LuaDeclGenerator;
    let output = haphe::generate(&LuaDeclGenerator::new(), &REGISTRY).expect("generation succeeds");
    let out = String::from_utf8(output.files[0].content.clone()).unwrap();
    assert!(
        out.contains("function Pair:first_of__i64(a, b) end"),
        "got:\n{out}"
    );
    assert!(
        out.contains("function Pair:first_of__string(a, b) end"),
        "got:\n{out}"
    );
    assert!(
        out.contains("function Pair:add_in__f64(value) end"),
        "got:\n{out}"
    );
    assert!(
        out.contains("function m.echo__i64(value) end"),
        "got:\n{out}"
    );
    assert!(
        out.contains("function m.echo__string(value) end"),
        "got:\n{out}"
    );
    // The substituted signature annotates each monomorph.
    assert!(out.contains("---@param a integer"), "got:\n{out}");
    assert!(out.contains("---@param a string"), "got:\n{out}");
}

#[cfg(not(feature = "generics"))]
#[test]
fn static_generic_methods_rejected_without_generics_feature() {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    let err = haphe_lua::bind_type::<Pair>(&lua, &table).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("first_of") || msg.contains("add_in"),
        "got: {msg}"
    );
    assert!(msg.contains("generic function"), "got: {msg}");
}

/// Async static generic monomorphs: same mangled entries, awaited bodies.
#[derive(Script, Clone)]
#[script(thread_safety = none, methods)]
struct AsyncPair {
    total: i64,
}

#[script]
impl AsyncPair {
    #[script(constructor)]
    fn new(total: i64) -> Self {
        AsyncPair { total }
    }

    #[script(instantiate(i64), instantiate(String))]
    async fn tag<T: ToString>(&self, value: T) -> String {
        #[cfg(feature = "async")]
        tokio::task::yield_now().await;
        value.to_string()
    }

    #[script(instantiate(i64))]
    async fn bump<T: Acc>(&mut self, value: T) {
        #[cfg(feature = "async")]
        tokio::task::yield_now().await;
        self.total += value.acc();
    }

    fn get(&self) -> i64 {
        self.total
    }
}

/// Async static generic free function.
#[script(instantiate(i64))]
async fn aecho<T>(value: T) -> T {
    #[cfg(feature = "async")]
    tokio::task::yield_now().await;
    value
}

#[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
mod with_async {
    use super::*;

    #[tokio::test]
    async fn async_monomorphs_round_trip_and_mut_writes_back() {
        let lua = Lua::new();
        let table = lua.create_table().unwrap();
        haphe_lua::bind_type::<AsyncPair>(&lua, &table).unwrap();
        lua.globals().set("AP", &table).unwrap();
        let out: String = lua
            .load("return AP.new(0):tag__i64(7)")
            .eval_async()
            .await
            .unwrap();
        assert_eq!(out, "7");
        let out: String = lua
            .load("return AP.new(0):tag__string('x')")
            .eval_async()
            .await
            .unwrap();
        assert_eq!(out, "x");
        let out: i64 = lua
            .load(
                "local p = AP.new(1)
                 p:bump__i64(9)
                 return p:get()",
            )
            .eval_async()
            .await
            .unwrap();
        assert_eq!(out, 10);
    }

    #[tokio::test]
    async fn async_free_fn_monomorph_round_trips() {
        let lua = Lua::new();
        let fns = lua.create_table().unwrap();
        haphe_lua::bind_fn::<aecho>(&lua, &fns).unwrap();
        lua.globals().set("fns", fns).unwrap();
        let out: i64 = lua
            .load("return fns.aecho__i64(5)")
            .eval_async()
            .await
            .unwrap();
        assert_eq!(out, 5);
    }
}
