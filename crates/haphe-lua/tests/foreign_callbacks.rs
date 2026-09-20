//! Foreign direction: `#[script(foreign)]` trait handles dispatching into a
//! Lua callbacks table.

use std::collections::HashMap;

use haphe::{ForeignError, Script, script};
use haphe_lua::{LuaBindError, foreign_handle};
use mlua::Lua;

/// Host-side hooks supplied by a Lua script.
#[script(foreign, thread_safety = none)]
pub trait Hooks {
    fn greet(&self, name: String) -> String;

    fn add(&self, a: i64, b: i64) -> i64;

    fn maybe(&self, value: Option<i64>) -> Option<i64>;

    fn nums(&self) -> Vec<i64>;

    fn stats(&self) -> HashMap<String, i64>;

    #[script(rename = "shout")]
    fn make_loud(&self, text: String) -> String;

    fn checked(&self, n: i64) -> Result<i64, HookError>;

    fn pick(&self, color: Color) -> Color;
}

#[derive(Debug)]
pub struct HookError(String);

impl From<ForeignError> for HookError {
    fn from(e: ForeignError) -> Self {
        Self(e.to_string())
    }
}

/// A named color.
#[derive(Script, Debug, PartialEq)]
pub enum Color {
    Red,
    DarkBlue,
}

const CALLBACKS: &str = r#"
return {
    greet = function(name) return "hi " .. name end,
    add = function(a, b) return a + b end,
    maybe = function(v) return v end,
    nums = function() return { 1, 2, 3 } end,
    stats = function() return { wins = 3, losses = 1 } end,
    shout = function(text) return string.upper(text) end,
    checked = function(n)
        if n < 0 then error("negative input") end
        return n * 2
    end,
    pick = function(color)
        if color == "Red" then return "DarkBlue" else return "Red" end
    end,
}
"#;

fn hooks_from(lua: &Lua, script: &str) -> Result<HooksHandle, LuaBindError> {
    let table: mlua::Table = lua.load(script).eval().unwrap();
    foreign_handle(lua, &table)
}

#[test]
fn callbacks_dispatch_with_shape_coverage() {
    let lua = Lua::new();
    let hooks = hooks_from(&lua, CALLBACKS).unwrap();

    assert_eq!(hooks.greet("ada".to_string()), "hi ada");
    assert_eq!(hooks.add(2, 40), 42);
    assert_eq!(hooks.maybe(Some(7)), Some(7));
    assert_eq!(hooks.maybe(None), None);
    assert_eq!(hooks.nums(), vec![1, 2, 3]);
    let stats = hooks.stats();
    assert_eq!(stats.get("wins"), Some(&3));
    assert_eq!(stats.get("losses"), Some(&1));
}

#[test]
fn renamed_method_is_keyed_by_exposed_name() {
    let lua = Lua::new();
    let hooks = hooks_from(&lua, CALLBACKS).unwrap();
    assert_eq!(hooks.make_loud("hey".to_string()), "HEY");
}

#[test]
fn unit_enums_round_trip_as_case_names() {
    let lua = Lua::new();
    let hooks = hooks_from(&lua, CALLBACKS).unwrap();
    // Case names are the declared exposed names, passed through verbatim in
    // both directions and matched exactly.
    assert_eq!(hooks.pick(Color::Red), Color::DarkBlue);
    assert_eq!(hooks.pick(Color::DarkBlue), Color::Red);
}

#[test]
#[should_panic(expected = "unexpected value")]
fn non_declared_case_spelling_fails_conversion() {
    let lua = Lua::new();
    let hooks = hooks_from(
        &lua,
        r#"
        return {
            greet = function(n) return n end,
            add = function(a, b) return a + b end,
            maybe = function(v) return v end,
            nums = function() return {} end,
            stats = function() return {} end,
            shout = function(t) return t end,
            checked = function(n) return n end,
            pick = function(c) return "dark-blue" end,
        }
        "#,
    )
    .unwrap();
    hooks.pick(Color::Red);
}

#[test]
fn lua_error_surfaces_through_result_methods() {
    let lua = Lua::new();
    let hooks = hooks_from(&lua, CALLBACKS).unwrap();
    assert_eq!(hooks.checked(21).unwrap(), 42);
    let HookError(message) = hooks.checked(-1).unwrap_err();
    assert!(message.contains("checked"), "got: {message}");
    assert!(message.contains("negative input"), "got: {message}");
}

#[test]
#[should_panic(expected = "foreign function `add` failed")]
fn lua_error_panics_through_non_result_methods() {
    let lua = Lua::new();
    let hooks = hooks_from(
        &lua,
        r#"
        return {
            greet = function(n) return n end,
            add = function() error("boom") end,
            maybe = function(v) return v end,
            nums = function() return {} end,
            stats = function() return {} end,
            shout = function(t) return t end,
            checked = function(n) return n end,
            pick = function(c) return c end,
        }
        "#,
    )
    .unwrap();
    hooks.add(1, 1);
}

#[test]
fn missing_callback_is_a_build_time_error() {
    let lua = Lua::new();
    let Err(err) = hooks_from(&lua, "return { greet = function(n) return n end }") else {
        panic!("expected an error");
    };
    match err {
        LuaBindError::MissingForeignFunction {
            interface,
            function,
        } => {
            assert_eq!(interface, "Hooks");
            assert_eq!(function, "add");
        }
        other => panic!("expected MissingForeignFunction, got: {other}"),
    }
}

// ---------------------------------------------------------------------------
// Generic foreign interfaces and methods
// ---------------------------------------------------------------------------

/// A host-backed store, generic over the value type.
#[script(foreign, thread_safety = none)]
pub trait Store<T> {
    fn get(&self, key: String) -> Option<T>;
}

/// Converts raw values, generic per method (STATIC dispatch: one Lua
/// handler per declared instantiation, addressed by the mangled name).
#[script(foreign)]
pub trait Conv {
    #[script(instantiate(i64), instantiate(String))]
    fn convert<U>(&self, raw: String) -> U;

    #[script(instantiate(i64))]
    fn parse<U>(&self, raw: String) -> Result<U, HookError>;
}

/// Renders values, generic per method (`dyn`: one erased Lua handler under
/// the plain name serves every instantiation).
#[script(foreign)]
pub trait Render {
    #[script(dyn, instantiate(i64), instantiate(String))]
    fn show<U>(&self, value: U) -> String;
}

const DYNAMIC_CALLBACKS: &str = r#"
return {
    get = function(key)
        if key == "n" then return 7 end
        if key == "s" then return "seven" end
        return nil
    end,
    convert__i64 = function(raw) return tonumber(raw) end,
    convert__string = function(raw) return raw .. "!" end,
    parse__i64 = function(raw) return tonumber(raw) end,
    show = function(value) return tostring(value) end,
}
"#;

#[cfg(not(feature = "generics"))]
#[test]
fn generic_foreign_interface_rejected_without_feature() {
    let lua = Lua::new();
    let table: mlua::Table = lua.load(DYNAMIC_CALLBACKS).eval().unwrap();
    let Err(err) = foreign_handle::<StoreHandle<i64>>(&lua, &table) else {
        panic!("expected an error");
    };
    assert!(
        matches!(err, LuaBindError::GenericForeignInterface { name: "Store" }),
        "got: {err}"
    );
}

#[cfg(not(feature = "generics"))]
#[test]
fn generic_foreign_method_rejected_without_feature() {
    let lua = Lua::new();
    let table: mlua::Table = lua.load(DYNAMIC_CALLBACKS).eval().unwrap();
    let Err(err) = foreign_handle::<ConvHandle>(&lua, &table) else {
        panic!("expected an error");
    };
    assert!(
        matches!(err, LuaBindError::GenericFunction { name: "convert" }),
        "got: {err}"
    );
}

/// Trait-level generics stay erased: one Lua table serves every
/// instantiation of the interface.
#[cfg(feature = "generics")]
#[test]
fn generic_interfaces_dispatch_dynamically() {
    let lua = Lua::new();
    let table: mlua::Table = lua.load(DYNAMIC_CALLBACKS).eval().unwrap();
    let ints: StoreHandle<i64> = foreign_handle(&lua, &table).unwrap();
    let strings: StoreHandle<String> = foreign_handle(&lua, &table).unwrap();
    assert_eq!(ints.get("n".to_string()), Some(7));
    assert_eq!(strings.get("s".to_string()), Some("seven".to_string()));
    assert_eq!(ints.get("missing".to_string()), None);
}

/// STATIC dispatch routes each Rust instantiation to its mangled handler.
#[cfg(feature = "generics")]
#[test]
fn static_generic_methods_route_to_mangled_handlers() {
    let lua = Lua::new();
    let table: mlua::Table = lua.load(DYNAMIC_CALLBACKS).eval().unwrap();
    let conv: ConvHandle = foreign_handle(&lua, &table).unwrap();
    let n: i64 = conv.convert("41".to_string());
    assert_eq!(n, 41);
    let s: String = conv.convert("x".to_string());
    assert_eq!(s, "x!");
}

/// A call whose type arguments match no declared instantiation fails
/// descriptively, listing the declared set.
#[cfg(feature = "generics")]
#[test]
fn undeclared_static_instantiation_errors_with_declared_set() {
    let lua = Lua::new();
    let table: mlua::Table = lua.load(DYNAMIC_CALLBACKS).eval().unwrap();
    let conv: ConvHandle = foreign_handle(&lua, &table).unwrap();
    let HookError(message) = conv.parse::<bool>("true".to_string()).unwrap_err();
    assert!(
        message.contains("no handler for instantiation `parse__bool`"),
        "got: {message}"
    );
    assert!(
        message.contains("declared instantiations (parse__i64)"),
        "got: {message}"
    );
}

/// A declared instantiation without its mangled handler fails at
/// construction, like any other missing callback.
#[cfg(feature = "generics")]
#[test]
fn missing_mangled_handler_is_a_build_time_error() {
    let lua = Lua::new();
    let table: mlua::Table = lua
        .load(
            r"
            return {
                convert__i64 = function(raw) return tonumber(raw) end,
                parse__i64 = function(raw) return tonumber(raw) end,
                show = function(value) return tostring(value) end,
            }
            ",
        )
        .eval()
        .unwrap();
    let Err(err) = foreign_handle::<ConvHandle>(&lua, &table) else {
        panic!("expected an error");
    };
    match err {
        LuaBindError::MissingForeignInstantiation {
            interface,
            function,
            mangled,
        } => {
            assert_eq!(interface, "Conv");
            assert_eq!(function, "convert");
            assert_eq!(mangled, "convert__string");
        }
        other => panic!("expected MissingForeignInstantiation, got: {other}"),
    }
}

/// `dyn`: one plain-named handler serves every Rust instantiation.
#[cfg(feature = "generics")]
#[test]
fn dyn_generic_methods_dispatch_through_one_handler() {
    let lua = Lua::new();
    let table: mlua::Table = lua.load(DYNAMIC_CALLBACKS).eval().unwrap();
    let render: RenderHandle = foreign_handle(&lua, &table).unwrap();
    assert_eq!(render.show(7i64), "7");
    assert_eq!(render.show("x".to_string()), "x");
}

// ---------------------------------------------------------------------------
// Async dispatch
// ---------------------------------------------------------------------------

#[cfg(feature = "async")]
mod async_dispatch {
    use super::*;

    /// Async host hooks.
    #[script(foreign, thread_safety = none)]
    pub trait AsyncHooks {
        async fn fetch(&self, url: String) -> Result<String, HookError>;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn async_method_dispatches_through_call_async() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let lua = Lua::new();
                let table: mlua::Table = lua
                    .load(r#"return { fetch = function(url) return "body of " .. url end }"#)
                    .eval()
                    .unwrap();
                let hooks: AsyncHooksHandle = foreign_handle(&lua, &table).unwrap();
                let body = hooks.fetch("http://example".to_string()).await.unwrap();
                assert_eq!(body, "body of http://example");
            })
            .await;
    }

    /// Async static generics route to mangled handlers like sync ones.
    #[cfg(feature = "generics")]
    #[script(foreign, thread_safety = none)]
    pub trait AsyncConv {
        #[script(instantiate(i64))]
        async fn fetch<U>(&self, key: String) -> Result<U, HookError>;
    }

    #[cfg(feature = "generics")]
    #[tokio::test(flavor = "current_thread")]
    async fn async_static_generic_routes_to_mangled_handler() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let lua = Lua::new();
                let table: mlua::Table = lua
                    .load(r"return { fetch__i64 = function(key) return #key end }")
                    .eval()
                    .unwrap();
                let conv: AsyncConvHandle = foreign_handle(&lua, &table).unwrap();
                let n: i64 = conv.fetch("abcd".to_string()).await.unwrap();
                assert_eq!(n, 4);
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn async_method_surfaces_lua_errors() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let lua = Lua::new();
                let table: mlua::Table = lua
                    .load(r#"return { fetch = function() error("offline") end }"#)
                    .eval()
                    .unwrap();
                let hooks: AsyncHooksHandle = foreign_handle(&lua, &table).unwrap();
                let HookError(message) =
                    hooks.fetch("http://example".to_string()).await.unwrap_err();
                assert!(message.contains("offline"), "got: {message}");
            })
            .await;
    }
}

// ---------------------------------------------------------------------------
// Empty containers round-trip across the foreign boundary
// ---------------------------------------------------------------------------

/// Container-shuttling hooks: Lua's `{}` is shape-ambiguous, and core's
/// empty-shape rule converts it on the RETURN path like arguments.
#[script(foreign, thread_safety = none)]
pub trait Shuttle {
    fn echo_list(&self, items: Vec<i64>) -> Vec<i64>;

    fn echo_map(&self, entries: HashMap<String, i64>) -> HashMap<String, i64>;

    fn empty_list(&self) -> Vec<i64>;

    fn empty_map(&self) -> HashMap<String, i64>;

    fn nested(&self) -> Vec<Vec<i64>>;

    fn list_by_key(&self) -> HashMap<String, Vec<i64>>;

    fn map_by_key(&self) -> HashMap<String, HashMap<String, i64>>;
}

const SHUTTLE_CALLBACKS: &str = r#"
return {
    echo_list = function(items)
        assert(type(items) == "table", "list arrives as a table")
        return items
    end,
    echo_map = function(entries)
        assert(type(entries) == "table", "map arrives as a table")
        return entries
    end,
    empty_list = function() return {} end,
    empty_map = function() return {} end,
    nested = function() return { {} } end,
    list_by_key = function() return { a = {}, b = { 1, 2 } } end,
    map_by_key = function() return { outer = {}, full = { x = 7 } } end,
}
"#;

#[test]
fn empty_containers_round_trip_across_the_foreign_boundary() {
    let lua = Lua::new();
    let table: mlua::Table = lua.load(SHUTTLE_CALLBACKS).eval().unwrap();
    let shuttle: ShuttleHandle = foreign_handle(&lua, &table).unwrap();

    // Outbound empties arrive as empty tables and echo back intact.
    assert_eq!(shuttle.echo_list(Vec::new()), Vec::<i64>::new());
    assert_eq!(shuttle.echo_map(HashMap::new()), HashMap::new());

    // Inbound `{}` returns convert per the DECLARED type.
    assert_eq!(shuttle.empty_list(), Vec::<i64>::new());
    assert_eq!(shuttle.empty_map(), HashMap::new());
    assert_eq!(shuttle.nested(), vec![Vec::<i64>::new()]);

    // Map-VALUED nesting on the foreign return path: the empty inner table
    // follows the declared value type, populated siblings undistorted.
    let by_key = shuttle.list_by_key();
    assert_eq!(by_key.get("a"), Some(&Vec::new()));
    assert_eq!(by_key.get("b"), Some(&vec![1, 2]));
    let deep = shuttle.map_by_key();
    assert_eq!(deep.get("outer"), Some(&HashMap::new()));
    assert_eq!(deep.get("full").and_then(|m| m.get("x")), Some(&7));

    // Populated values are never touched by the empty-shape rule.
    assert_eq!(shuttle.echo_list(vec![1, 2]), vec![1, 2]);
    let mut entries = HashMap::new();
    entries.insert("a".to_string(), 1);
    assert_eq!(shuttle.echo_map(entries.clone()), entries);
}
