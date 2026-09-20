//! Associated fns (no receiver) bind through the dedicated `associated*`
//! channels onto the TYPE TABLE, next to constructors: `Type.assoc(...)`
//! needs no instance, and the fn never appears on instances.

#![allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    clippy::doc_markdown,
    reason = "fixture shapes are dictated by the bridge surface under test: `#[script]` functions receive OWNED values (the boundary contract) and docs name fixture idents verbatim"
)]
#![allow(
    dead_code,
    reason = "fixtures are exercised through their generated descriptors and bridge wrappers, not direct calls"
)]

use haphe::{Script, script};
use haphe_lua::bind_type;
use mlua::Lua;

/// A transparent bool newtype: not on the syntactic whitelist, so
/// receiver-less fns over it register through trait-presence dispatch.
#[derive(Script, Clone, Copy)]
#[script(transparent)]
struct Flag(bool);

#[derive(Script, Clone)]
#[script(thread_safety = send_sync, methods)]
struct Meter {
    reading: i64,
}

#[script]
impl Meter {
    #[script(constructor)]
    fn new(reading: i64) -> Self {
        Meter { reading }
    }

    fn read(&self) -> i64 {
        self.reading
    }

    /// Sync associated fn: doubles a seed without any instance.
    fn splat(seed: i64) -> i64 {
        seed * 2
    }

    /// Fallible associated fn: the `Result` error surfaces as a Lua error
    /// tagged with the declared kind.
    #[script(error_kind = "ParseError")]
    fn parse(text: String) -> Result<i64, TextError> {
        text.trim()
            .parse::<i64>()
            .map_err(|e| TextError(e.to_string()))
    }

    /// Newtype-typed receiver-less fn: registered via trait-presence
    /// dispatch onto the `associated` channel.
    fn flipped(flag: Flag) -> Flag {
        Flag(!flag.0)
    }

    /// Fallible dispatched associated fn: `Err` crosses as a Callee error
    /// tagged with the declared kind.
    #[script(error_kind = "MakeError")]
    fn checked_flip(flag: Flag) -> Result<Flag, TextError> {
        if flag.0 {
            Ok(Flag(false))
        } else {
            Err(TextError("already off".into()))
        }
    }
}

fn env(lua: &Lua) {
    let tbl = lua.create_table().unwrap();
    bind_type::<Meter>(lua, &tbl).unwrap();
    lua.globals().set("Meter", &tbl).unwrap();
    haphe_lua::install_error_info(lua).unwrap();
}

#[test]
fn associated_fn_called_on_type_table_without_instance() {
    let lua = Lua::new();
    env(&lua);
    let out: i64 = lua.load("return Meter.splat(21)").eval().unwrap();
    assert_eq!(out, 42);
}

#[test]
fn fallible_associated_fn_ok_and_err() {
    let lua = Lua::new();
    env(&lua);
    let out: i64 = lua.load("return Meter.parse(' 7 ')").eval().unwrap();
    assert_eq!(out, 7);
    let err = lua
        .load("return Meter.parse('x')")
        .eval::<i64>()
        .unwrap_err();
    assert!(err.to_string().contains("ParseError"), "{err}");
}

#[test]
fn associated_fn_is_not_an_instance_method() {
    let lua = Lua::new();
    env(&lua);
    lua.load("m = Meter.new(5)").exec().unwrap();
    // Instance methods still work.
    let read: i64 = lua.load("return m:read()").eval().unwrap();
    assert_eq!(read, 5);
    // The associated fn does not exist on the instance.
    let err = lua.load("return m:splat(1)").eval::<i64>().unwrap_err();
    assert!(
        err.to_string().contains("splat"),
        "expected an unknown-method error, got: {err}"
    );
}

#[test]
fn dispatched_associated_fn_called_without_instance() {
    let lua = Lua::new();
    env(&lua);
    let (out, is_bool): (bool, bool) = lua
        .load("local v = Meter.flipped(false) return v, type(v) == 'boolean'")
        .eval()
        .unwrap();
    assert!(out);
    assert!(is_bool, "crosses as a native boolean");
    // Not on instances either.
    lua.load("m = Meter.new(5)").exec().unwrap();
    let err = lua
        .load("return m:flipped(true)")
        .eval::<bool>()
        .unwrap_err();
    assert!(
        err.to_string().contains("flipped"),
        "expected an unknown-method error, got: {err}"
    );
}

#[test]
fn fallible_dispatched_associated_fn_ok_and_err() {
    let lua = Lua::new();
    env(&lua);
    let out: bool = lua.load("return Meter.checked_flip(true)").eval().unwrap();
    assert!(!out);
    let (ok, rendered, kind): (bool, String, String) = lua
        .load(
            "local ok, e = pcall(function() return Meter.checked_flip(false) end) \
             local info = haphe_error(e) \
             return ok, tostring(e), info.kind",
        )
        .eval()
        .unwrap();
    assert!(!ok);
    assert!(
        rendered.contains("MakeError: already off"),
        "kind-prefixed message, got {rendered:?}"
    );
    assert_eq!(kind, "MakeError");
}

#[cfg(all(feature = "async", not(feature = "send")))]
mod with_async {
    use super::*;

    #[derive(Script, Clone)]
    #[script(thread_safety = none, methods)]
    struct Fetcher {
        hits: i64,
    }

    #[script]
    impl Fetcher {
        /// Async associated fn: awaited through the type table.
        async fn lookup(path: String) -> String {
            format!("db/{path}")
        }

        /// Newtype-typed async receiver-less fn: `associated_async` via
        /// trait-presence dispatch.
        async fn flip_later(flag: Flag) -> Flag {
            Flag(!flag.0)
        }
    }

    #[tokio::test]
    async fn async_associated_fn_called_without_instance() {
        let lua = Lua::new();
        let tbl = lua.create_table().unwrap();
        bind_type::<Fetcher>(&lua, &tbl).unwrap();
        lua.globals().set("Fetcher", &tbl).unwrap();
        let out: String = lua
            .load("return Fetcher.lookup('users')")
            .eval_async()
            .await
            .unwrap();
        assert_eq!(out, "db/users");
    }

    #[tokio::test]
    async fn dispatched_async_associated_fn_called_without_instance() {
        let lua = Lua::new();
        let tbl = lua.create_table().unwrap();
        bind_type::<Fetcher>(&lua, &tbl).unwrap();
        lua.globals().set("Fetcher", &tbl).unwrap();
        let (out, is_bool): (bool, bool) = lua
            .load("local v = Fetcher.flip_later(false) return v, type(v) == 'boolean'")
            .eval_async()
            .await
            .unwrap();
        assert!(out);
        assert!(is_bool, "crosses as a native boolean");
    }
}

#[cfg(feature = "generics")]
mod with_generics {
    use super::*;

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

    #[derive(Script, Clone)]
    #[script(thread_safety = send_sync, methods)]
    struct Tally {
        base: i64,
    }

    #[script]
    impl Tally {
        /// Static generic associated fn: one mangled type-table entry per
        /// instantiation (`first__i64`, `first__string`).
        #[script(instantiate(i64), instantiate(String))]
        fn first<T>(a: T, b: T) -> T {
            let _ = b;
            a
        }

        /// Dyn generic associated fn: one type-table callable scans the
        /// candidates by value shape.
        #[script(dyn, instantiate(i64), instantiate(f64))]
        fn total<T: Acc>(value: T) -> i64 {
            value.acc()
        }
    }

    fn env(lua: &Lua) {
        let tbl = lua.create_table().unwrap();
        bind_type::<Tally>(lua, &tbl).unwrap();
        lua.globals().set("Tally", &tbl).unwrap();
    }

    #[test]
    fn static_generic_associated_fn_dispatches_by_mangled_name() {
        let lua = Lua::new();
        env(&lua);
        let out: i64 = lua.load("return Tally.first__i64(4, 9)").eval().unwrap();
        assert_eq!(out, 4);
        let out: String = lua
            .load("return Tally.first__string('a', 'b')")
            .eval()
            .unwrap();
        assert_eq!(out, "a");
    }

    #[test]
    fn dyn_associated_fn_scans_candidates_by_value_type() {
        let lua = Lua::new();
        env(&lua);
        let out: i64 = lua.load("return Tally.total(7)").eval().unwrap();
        assert_eq!(out, 7);
        // A float picks the f64 candidate (truncating acc).
        let out: i64 = lua.load("return Tally.total(2.9)").eval().unwrap();
        assert_eq!(out, 2);
        // No candidate accepts a table: descriptive no-match error.
        let err = lua
            .load("return Tally.total({})")
            .eval::<i64>()
            .unwrap_err();
        assert!(err.to_string().contains("total"), "{err}");
    }
}

#[cfg(not(feature = "generics"))]
mod without_generics {
    use super::*;

    #[derive(Script, Clone)]
    #[script(thread_safety = send_sync, methods)]
    struct Gen {
        n: i64,
    }

    #[script]
    impl Gen {
        #[script(instantiate(i64))]
        fn pick<T>(value: T) -> T {
            value
        }
    }

    /// Without the `generics` feature the generic associated registration is
    /// rejected loudly, never silently dropped.
    #[test]
    fn generic_associated_fn_rejected_without_feature() {
        let lua = Lua::new();
        let tbl = lua.create_table().unwrap();
        let err = bind_type::<Gen>(&lua, &tbl).unwrap_err();
        assert!(
            matches!(err, haphe_lua::LuaBindError::GenericFunction { name } if name == "pick"),
            "expected GenericFunction, got {err}"
        );
    }
}

/// A message-only fixture error: `String` itself no longer crosses (host
/// errors must implement `std::error::Error`).
#[derive(Debug)]
struct TextError(String);

impl std::fmt::Display for TextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TextError {}
