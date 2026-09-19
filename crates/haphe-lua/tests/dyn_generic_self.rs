//! Dyn methods on GENERIC self types: each candidate carries the self
//! instantiation, so ranking substitutes the type's parameters
//! (`GenericParam("T")`) alongside the method's own.

#![cfg(feature = "generics")]
#![allow(dead_code)]

use haphe::{Script, script};
use haphe_lua::bind_type;
use mlua::Lua;

#[derive(Script, Clone)]
#[script(thread_safety = send_sync, methods)]
struct Pair<T: Clone + Send + Sync + 'static> {
    left: T,
    right: T,
}

#[script]
impl<T: Clone + Send + Sync + 'static> Pair<T> {
    #[script(constructor)]
    fn new(left: T, right: T) -> Self {
        Pair { left, right }
    }

    /// Bare `dyn` on the method's own parameter: default candidate set.
    #[script(dyn)]
    fn tag_with<U>(&self, marker: U) -> U {
        marker
    }

    /// Names the instantiation that handled the call.
    #[script(dyn)]
    fn kind<U>(&self, marker: U) -> String {
        let _ = marker;
        std::any::type_name::<U>().to_string()
    }

    /// The FIRST parameter references the SELF type's `T`: without the
    /// merged substitution it ranks `No` for every candidate and dispatch
    /// degrades to declaration-order try-calling (which would pick f64 for
    /// an integer pair). With it, the i64 candidate ranks Exact and wins.
    #[script(dyn, instantiate(f64), instantiate(i64))]
    fn pick<U>(&self, anchor: T, marker: U) -> String {
        let _ = (anchor, marker);
        std::any::type_name::<U>().to_string()
    }

    /// `&mut self` with a self-typed parameter: mutation writes back.
    #[script(dyn, instantiate(bool))]
    fn replace_left<U>(&mut self, value: T, flag: U) -> U {
        self.left = value;
        flag
    }

    /// STATIC generic method on a generic self type: one mangled monomorph
    /// per instantiation on the per-monomorph metatable (the self identity
    /// is implicit — no dyn scan involved).
    #[script(instantiate(f64))]
    fn sized<U>(&self, scale: U) -> U {
        scale
    }

    fn first(&self) -> T {
        self.left.clone()
    }
}

fn setup() -> Lua {
    let lua = Lua::new();
    let tbl = lua.create_table().unwrap();
    bind_type::<Pair<i64>>(&lua, &tbl).unwrap();
    lua.globals().set("PairI64", tbl).unwrap();
    lua
}

#[test]
fn dyn_methods_resolve_on_generic_self_userdata() {
    let lua = setup();
    let out: i64 = lua
        .load("return PairI64.new(1, 2):tag_with(5)")
        .eval()
        .unwrap();
    assert_eq!(out, 5);
    let out: String = lua
        .load("return PairI64.new(1, 2):tag_with('hello')")
        .eval()
        .unwrap();
    assert_eq!(out, "hello");
    let out: String = lua
        .load("return PairI64.new(1, 2):kind(true)")
        .eval()
        .unwrap();
    assert_eq!(out, "bool");
}

#[test]
fn self_typed_parameters_rank_through_the_merged_substitution() {
    let lua = setup();
    // Both candidates accept `(1, 2)` by try-calling (2 converts to f64 and
    // to i64), so only RANKING with `T = i64` resolved can pick the i64
    // candidate over the first-declared f64 one.
    let out: String = lua
        .load("return PairI64.new(1, 2):pick(1, 2)")
        .eval()
        .unwrap();
    assert_eq!(out, "i64");
}

#[test]
fn mut_dyn_method_with_self_typed_param_writes_back() {
    let lua = setup();
    let out: i64 = lua
        .load(
            "local p = PairI64.new(1, 2)
             assert(p:replace_left(9, true) == true)
             return p:first()",
        )
        .eval()
        .unwrap();
    assert_eq!(out, 9);
}

#[test]
fn no_match_lists_substituted_candidate_signatures() {
    let lua = setup();
    let err = lua
        .load("return PairI64.new(1, 2):replace_left('nope', {})")
        .exec()
        .unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("no dyn candidate of `replace_left`"),
        "unexpected error: {text}"
    );
    // The self-typed parameter renders through its monomorph.
    assert!(text.contains("value: i64"), "unexpected error: {text}");
}

#[test]
fn static_generic_method_monomorph_binds_on_generic_self() {
    let lua = setup();
    let out: f64 = lua
        .load("return PairI64.new(1, 2):sized__f64(2.5)")
        .eval()
        .unwrap();
    assert_eq!(out, 2.5);
    // Only the mangled monomorph exists; the plain name is absent.
    let err = lua
        .load("return PairI64.new(1, 2):sized(2.5)")
        .exec()
        .unwrap_err();
    assert!(err.to_string().contains("sized"), "got: {err}");
}

/// Decl stubs keep the erased-class convention: the self type emits ONE
/// class, its parameters typed `any`, and a dyn method's self-typed
/// parameter renders `any` while the method's own instantiations drive the
/// signature and overloads.
#[test]
fn decl_stubs_keep_the_erased_class_convention() {
    haphe::registry! {
        static PAIR_REGISTRY = {
            structs: [Pair<i64>],
        };
    }
    let output = haphe::generate(&haphe_lua::LuaDeclGenerator::new(), &PAIR_REGISTRY).unwrap();
    let s = String::from_utf8(output.files[0].content.clone()).unwrap();
    assert!(s.contains("---@param anchor any"), "got:\n{s}");
    // Static generic methods stub per mangled monomorph.
    assert!(s.contains("sized__f64"), "got:\n{s}");
    assert!(s.contains("---@param scale number"), "got:\n{s}");
    assert!(
        s.contains("---@overload fun(anchor: any, marker: integer): string")
            || s.contains("---@param marker number"),
        "got:\n{s}"
    );
}
