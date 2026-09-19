//! `TypeDescriptor::Borrowed` (lifetime carriers like `Cow`): Lua has no
//! borrow semantics, so values cross as the inner type (cloned at the
//! boundary), decl stubs render the inner type, and dyn candidate listings
//! show the inner type. The carried lifetime is ignored.

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

use std::borrow::Cow;

use haphe::{Script, script};
use mlua::Lua;

#[derive(Script, Clone)]
#[script(methods)]
struct Tag {
    name: String,
}

#[script]
impl Tag {
    #[script(constructor)]
    fn new(name: String) -> Self {
        Tag { name }
    }

    /// A `Cow` parameter with a named signature lifetime.
    fn suffixed(&self, base: Cow<'_, str>) -> String {
        format!("{base}-{}", self.name)
    }

    /// A `Cow` return borrowing the receiver.
    fn label(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.name)
    }
}

/// A free function taking and returning `Cow`.
#[script]
fn shout(text: Cow<'_, str>) -> Cow<'_, str> {
    Cow::Owned(text.to_uppercase())
}

/// Dyn candidates with a `Borrowed` parameter alongside the generic one.
#[cfg(feature = "generics")]
#[script(dyn, instantiate(i64), instantiate(String))]
fn brand<T>(value: T, label: Cow<'_, str>) -> String {
    let _ = value;
    format!("{label}:{}", std::any::type_name::<T>())
}

#[cfg(feature = "generics")]
haphe::registry! {
    pub static REGISTRY = {
        structs: [Tag],
        modules: [
            mod m {
                functions: [shout, brand],
                types: [Tag],
            },
        ],
    };
}

#[cfg(not(feature = "generics"))]
haphe::registry! {
    pub static REGISTRY = {
        structs: [Tag],
        modules: [
            mod m {
                functions: [shout],
                types: [Tag],
            },
        ],
    };
}

fn bound_lua() -> Lua {
    use haphe::RuntimeBinder;
    let mut lua = Lua::new();
    let binder = haphe_lua::LuaBinder::new();
    let validated = REGISTRY.validate().expect("registry validates");
    binder.bind(&validated, &mut lua).expect("binding succeeds");
    let table: mlua::Table = lua.load("return m").eval().expect("module table");
    haphe_lua::bind_fn::<shout>(&lua, &table).expect("shout binds");
    #[cfg(feature = "generics")]
    haphe_lua::bind_fn::<brand>(&lua, &table).expect("brand binds");
    let tag_table: mlua::Table = lua.load("return m.Tag").eval().expect("type table");
    haphe_lua::bind_type::<Tag>(&lua, &tag_table).expect("Tag binds");
    lua
}

#[test]
fn cow_free_fn_round_trips_as_string() {
    let lua = bound_lua();
    let out: String = lua.load("return m.shout('hey')").eval().unwrap();
    assert_eq!(out, "HEY");
}

#[test]
fn cow_method_param_and_return_round_trip() {
    let lua = bound_lua();
    let out: String = lua
        .load("return m.Tag.new('x'):suffixed('base')")
        .eval()
        .unwrap();
    assert_eq!(out, "base-x");
    let out: String = lua.load("return m.Tag.new('x'):label()").eval().unwrap();
    assert_eq!(out, "x");
}

#[test]
fn decl_renders_borrowed_as_inner_type() {
    let output = haphe::generate(&haphe_lua::LuaDeclGenerator::new(), &REGISTRY)
        .expect("generation succeeds");
    let s = String::from_utf8(output.files[0].content.clone()).unwrap();
    assert!(s.contains("---@param text string"), "got:\n{s}");
    assert!(s.contains("---@param base string"), "got:\n{s}");
    // `label`'s Cow return renders as string.
    assert!(
        s.contains("---@return string\nfunction Tag:label()"),
        "got:\n{s}"
    );
}

#[cfg(feature = "generics")]
#[test]
fn dyn_candidate_listing_renders_borrowed_as_inner_type() {
    let lua = bound_lua();
    let err = lua
        .load("return m.brand(true, 'x')")
        .eval::<String>()
        .unwrap_err()
        .to_string();
    assert!(err.contains("no dyn candidate of `brand`"), "got: {err}");
    assert!(
        err.contains("brand<i64>(value: i64, label: string)"),
        "got: {err}"
    );
    assert!(
        err.contains("brand<string>(value: string, label: string)"),
        "got: {err}"
    );
}
