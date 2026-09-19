//! Dyn-dispatched generic methods: one Lua method per name scans the
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
use mlua::Lua;

#[derive(Script, Clone)]
#[script(methods)]
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

    /// Bare `dyn`: the default candidate set {i64, f64, bool, String, char}.
    #[script(dyn)]
    fn mirror<T>(&self, value: T) -> T {
        value
    }

    /// Names the instantiation that handled the call.
    #[script(dyn)]
    fn kind<T>(&self, value: T) -> String {
        let _ = value;
        std::any::type_name::<T>().to_string()
    }

    /// Declaration-order tiebreak fixture: two float candidates.
    #[script(dyn, instantiate(f64), instantiate(f32))]
    fn floaty<T>(&self, value: T) -> String {
        let _ = value;
        std::any::type_name::<T>().to_string()
    }

    /// `&mut self` dyn method: mutation must write back to the userdata.
    #[script(dyn, instantiate(i64), instantiate(f64))]
    fn accumulate<T: Acc>(&mut self, value: T) {
        self.total += value.acc();
    }

    fn get(&self) -> i64 {
        self.total
    }
}

/// A methods-bearing enum with a dyn method.
#[derive(Script, Clone, Copy)]
#[script(methods)]
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
fn bound_lua() -> Lua {
    use haphe::RuntimeBinder;
    let mut lua = Lua::new();
    let binder = haphe_lua::LuaBinder::new();
    let validated = REGISTRY.validate().expect("registry validates");
    binder.bind(&validated, &mut lua).expect("binding succeeds");
    let holder_table: mlua::Table = lua.load("return m.Holder").eval().expect("type table");
    haphe_lua::bind_type::<Holder>(&lua, &holder_table).expect("Holder binds");
    let mode_table: mlua::Table = lua.load("return m.Mode").eval().expect("enum table");
    haphe_lua::bind_enum_type::<Mode>(&lua, &mode_table).expect("Mode binds");
    lua
}

#[cfg(feature = "generics")]
#[test]
fn bare_dyn_method_round_trips_default_candidates() {
    let lua = bound_lua();
    let out: i64 = lua.load("return m.Holder.new(0):mirror(5)").eval().unwrap();
    assert_eq!(out, 5);
    let out: f64 = lua
        .load("return m.Holder.new(0):mirror(1.5)")
        .eval()
        .unwrap();
    assert_eq!(out, 1.5);
    let out: String = lua
        .load("return m.Holder.new(0):mirror('hello')")
        .eval()
        .unwrap();
    assert_eq!(out, "hello");
    let out: String = lua.load("return m.Holder.new(0):kind(5)").eval().unwrap();
    assert_eq!(out, "i64");
    let out: String = lua
        .load("return m.Holder.new(0):kind(true)")
        .eval()
        .unwrap();
    assert_eq!(out, "bool");
}

#[cfg(feature = "generics")]
#[test]
fn coercible_pass_ties_break_by_declaration_order() {
    let lua = bound_lua();
    // An integer is Coercible to both float candidates; f64 is declared
    // first. A float is Exact for both; still first-declared.
    let out: String = lua.load("return m.Holder.new(0):floaty(5)").eval().unwrap();
    assert_eq!(out, "f64");
    let out: String = lua
        .load("return m.Holder.new(0):floaty(1.5)")
        .eval()
        .unwrap();
    assert_eq!(out, "f64");
}

#[cfg(feature = "generics")]
#[test]
fn mut_dyn_method_writes_back_to_the_userdata() {
    let lua = bound_lua();
    let out: i64 = lua
        .load(
            "local h = m.Holder.new(1)
             h:accumulate(5)
             h:accumulate(2.0)
             return h:get()",
        )
        .eval()
        .unwrap();
    assert_eq!(out, 8);
}

#[cfg(feature = "generics")]
#[test]
fn no_match_lists_candidate_signatures() {
    let lua = bound_lua();
    // A boolean matches no float candidate; every wrapper rejects it and
    // the error names each candidate signature.
    let err = lua
        .load("return m.Holder.new(0):floaty(true)")
        .eval::<String>()
        .unwrap_err()
        .to_string();
    assert!(err.contains("no dyn candidate of `floaty`"), "got: {err}");
    assert!(err.contains("floaty<f64>(value: f64)"), "got: {err}");
    assert!(err.contains("floaty<f32>(value: f32)"), "got: {err}");
}

#[cfg(feature = "generics")]
#[test]
fn enum_dyn_method_dispatches_on_userdata() {
    let lua = bound_lua();
    let fast = lua.create_any_userdata(Mode::Fast).unwrap();
    let slow = lua.create_any_userdata(Mode::Slow).unwrap();
    lua.globals().set("fast", fast).unwrap();
    lua.globals().set("slow", slow).unwrap();
    let out: i64 = lua.load("return fast:pick(1, 2)").eval().unwrap();
    assert_eq!(out, 1);
    let out: bool = lua.load("return slow:pick(true, false)").eval().unwrap();
    assert!(!out);
}

#[cfg(not(feature = "generics"))]
#[test]
fn dyn_methods_rejected_without_generics_feature() {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    let err = haphe_lua::bind_type::<Holder>(&lua, &table).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("dyn function `mirror`"), "got: {msg}");
    assert!(msg.contains("`generics` feature"), "got: {msg}");
}

#[test]
fn dyn_method_capability_gates_via_check() {
    use haphe::{BackendCapabilities, CompatibilityError};
    let validated = REGISTRY.validate().unwrap();
    BackendCapabilities::ALL.check(&validated).unwrap();
    let errors = BackendCapabilities::ALL
        .with_dyn_generics(false)
        .check(&validated)
        .unwrap_err();
    for method in ["mirror", "accumulate", "pick"] {
        assert!(
            errors.iter().any(|e| matches!(
                e,
                CompatibilityError::DynGenericsUnsupported { function } if *function == method
            )),
            "missing {method}: {errors:?}"
        );
    }
}

#[cfg(feature = "generics")]
#[test]
fn decl_stub_emits_method_overloads() {
    use haphe_lua::LuaDeclGenerator;
    let output = haphe::generate(&LuaDeclGenerator::new(), &REGISTRY).expect("generation succeeds");
    let out = String::from_utf8(output.files[0].content.clone()).unwrap();
    // Bare-dyn mirror: first default candidate (i64) is the primary
    // signature; the other four are overloads.
    assert!(out.contains("---@param value integer"), "got:\n{out}");
    assert!(
        out.contains("function Holder:mirror(value) end"),
        "got:\n{out}"
    );
    assert!(
        out.contains("---@overload fun(value: string): string"),
        "got:\n{out}"
    );
    assert!(
        out.contains("---@overload fun(value: number): number"),
        "got:\n{out}"
    );
    // Explicit instantiations on the mut method.
    assert!(
        out.contains("function Holder:accumulate(value) end"),
        "got:\n{out}"
    );
}

/// Async dyn methods: the same scan drives async candidates; `&mut self`
/// futures hold the guard, so mutation persists.
#[derive(Script, Clone)]
#[script(thread_safety = none, methods)]
struct AsyncHolder {
    total: i64,
}

#[script]
impl AsyncHolder {
    #[script(constructor)]
    fn new(total: i64) -> Self {
        AsyncHolder { total }
    }

    #[script(dyn, instantiate(i64), instantiate(String))]
    async fn tag<T: ToString>(&self, value: T) -> String {
        #[cfg(feature = "async")]
        tokio::task::yield_now().await;
        value.to_string()
    }

    #[script(dyn, instantiate(i64))]
    async fn bump<T: Acc>(&mut self, value: T) {
        #[cfg(feature = "async")]
        tokio::task::yield_now().await;
        self.total += value.acc();
    }

    fn get(&self) -> i64 {
        self.total
    }
}

#[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
mod with_async {
    use super::*;

    #[tokio::test]
    async fn async_dyn_method_picks_candidate_and_mut_writes_back() {
        let lua = Lua::new();
        let table = lua.create_table().unwrap();
        haphe_lua::bind_type::<AsyncHolder>(&lua, &table).unwrap();
        lua.globals().set("AH", &table).unwrap();
        let out: String = lua
            .load("return AH.new(0):tag(7)")
            .eval_async()
            .await
            .unwrap();
        assert_eq!(out, "7");
        let out: String = lua
            .load("return AH.new(0):tag('x')")
            .eval_async()
            .await
            .unwrap();
        assert_eq!(out, "x");
        let out: i64 = lua
            .load(
                "local h = AH.new(1)
                 h:bump(9)
                 return h:get()",
            )
            .eval_async()
            .await
            .unwrap();
        assert_eq!(out, 10);
    }
}
