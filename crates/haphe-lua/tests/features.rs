//! Tests for opt-in feature flags.
//!
//! Each section is gated on its feature and validates that the feature
//! actually works through haphe-lua's passthrough. Run specific features:
//!
//! ```sh
//! cargo test -p haphe-lua --test features --features serialize
//! cargo test -p haphe-lua --test features --features macros
//! cargo test -p haphe-lua --test features --features anyhow
//! cargo test -p haphe-lua --test features --features "send,error-send"
//! ```

#![allow(
    dead_code,
    reason = "fixtures are exercised through their generated descriptors and bridge wrappers, not direct calls"
)]

use haphe::{RuntimeBinder, Script, script};
use haphe_lua::LuaBinder;
use mlua::Lua;

// ---------------------------------------------------------------------------
// Shared fixtures
// ---------------------------------------------------------------------------

#[derive(Script)]
#[script(thread_safety = send_sync)]
struct Coord {
    x: f64,
    y: f64,
}

#[script]
fn noop() {}

haphe::registry! {
    pub static REGISTRY = {
        structs: [Coord],
        modules: [
            mod geo {
                functions: [noop],
                types: [Coord],
                constants: [
                    SCALE: f64 = 1.5,
                    LABEL: &str = "origin",
                    COUNT: i32 = 42,
                    ON: bool = true,
                ],
            },
        ],
    };
}

fn bound_lua() -> Lua {
    let mut lua = Lua::new();
    let binder = LuaBinder::new();
    let validated = REGISTRY.validate().unwrap();
    binder.bind(&validated, &mut lua).unwrap();
    lua
}

// ---------------------------------------------------------------------------
// error-send: mlua errors are Send
// ---------------------------------------------------------------------------

#[cfg(feature = "error-send")]
mod error_send {
    use super::*;

    fn assert_send<T: Send>() {}

    #[test]
    fn mlua_error_is_send() {
        assert_send::<mlua::Error>();
    }

    #[test]
    fn lua_bind_error_is_send() {
        assert_send::<haphe_lua::LuaBindError>();
    }
}

// ---------------------------------------------------------------------------
// serialize: serde integration for Lua values
// ---------------------------------------------------------------------------

#[cfg(feature = "serialize")]
mod serialize {
    use super::*;
    use mlua::LuaSerdeExt;

    #[test]
    fn rust_struct_to_lua_table() {
        let lua = Lua::new();

        #[derive(serde::Serialize)]
        struct Point {
            x: f64,
            y: f64,
        }

        let val = lua.to_value(&Point { x: 1.0, y: 2.0 }).unwrap();
        let table: mlua::Table = mlua::FromLua::from_lua(val, &lua).unwrap();
        let x: f64 = table.get("x").unwrap();
        let y: f64 = table.get("y").unwrap();
        assert!((x - 1.0).abs() < 1e-12);
        assert!((y - 2.0).abs() < 1e-12);
    }

    #[test]
    fn lua_table_to_rust_struct() {
        let lua = Lua::new();

        #[derive(serde::Deserialize, Debug, PartialEq)]
        struct Config {
            name: String,
            value: i64,
        }

        let table = lua.load(r#"{ name = "test", value = 99 }"#).eval().unwrap();
        let config: Config = lua.from_value(table).unwrap();
        assert_eq!(
            config,
            Config {
                name: "test".into(),
                value: 99,
            }
        );
    }

    #[test]
    fn bound_constants_roundtrip_via_serde() {
        let lua = bound_lua();

        // Read bound constants from Lua and deserialize into a Rust map.
        let table: mlua::Value = lua
            .load(r#"{ scale = geo.SCALE, label = geo.LABEL, count = geo.COUNT }"#)
            .eval()
            .unwrap();

        #[derive(serde::Deserialize, Debug)]
        struct GeoInfo {
            scale: f64,
            label: String,
            count: i64,
        }

        let info: GeoInfo = lua.from_value(table).unwrap();
        assert!((info.scale - 1.5).abs() < 1e-12);
        assert_eq!(info.label, "origin");
        assert_eq!(info.count, 42);
    }

    #[test]
    fn null_value() {
        let lua = Lua::new();
        lua.globals().set("null", lua.null()).unwrap();

        let val: mlua::Value = lua.load("null").eval().unwrap();
        let opt: Option<String> = lua.from_value(val).unwrap();
        assert_eq!(opt, None);
    }
}

// ---------------------------------------------------------------------------
// macros: chunk! macro for inline Lua with Rust variable interpolation
// ---------------------------------------------------------------------------

#[cfg(feature = "macros")]
mod macros {
    use super::*;
    use mlua::chunk;

    #[test]
    fn chunk_macro_basic() {
        let lua = Lua::new();
        let x = 10i64;
        let result: i64 = lua.load(chunk! { return $x + 5 }).eval().unwrap();
        assert_eq!(result, 15);
    }

    #[test]
    fn chunk_macro_with_bound_state() {
        let lua = bound_lua();
        let multiplier = 3.0f64;
        let result: f64 = lua
            .load(chunk! { return geo.SCALE * $multiplier })
            .eval()
            .unwrap();
        assert!((result - 4.5).abs() < 1e-12);
    }

    #[test]
    fn chunk_macro_multiline() {
        let lua = bound_lua();
        let result: String = lua
            .load(chunk! {
                local prefix = "value="
                local n = geo.COUNT
                return prefix .. tostring(n)
            })
            .eval()
            .unwrap();
        assert_eq!(result, "value=42");
    }
}

// ---------------------------------------------------------------------------
// anyhow: anyhow::Error converts into mlua::Error
// ---------------------------------------------------------------------------

#[cfg(feature = "anyhow")]
mod anyhow_feature {
    use super::*;

    #[test]
    fn anyhow_error_into_lua_error() {
        let err = anyhow::anyhow!("something went wrong");
        let lua_err: mlua::Error = err.into();
        let msg = lua_err.to_string();
        assert!(msg.contains("something went wrong"), "got: {msg}");
    }

    #[test]
    fn anyhow_in_lua_callback() {
        let lua = Lua::new();

        let func = lua
            .create_function(|_, val: i64| -> mlua::Result<i64> {
                if val < 0 {
                    Err(anyhow::anyhow!("negative value: {val}").into())
                } else {
                    Ok(val * 2)
                }
            })
            .unwrap();
        lua.globals().set("double_positive", func).unwrap();

        let ok: i64 = lua.load("return double_positive(5)").eval().unwrap();
        assert_eq!(ok, 10);

        let err = lua
            .load("return double_positive(-1)")
            .eval::<i64>()
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("negative value: -1"), "got: {msg}");
    }

    // anyhow implies error-send
    #[test]
    fn anyhow_implies_error_send() {
        fn assert_send<T: Send>() {}
        assert_send::<mlua::Error>();
    }
}

// ---------------------------------------------------------------------------
// send: Lua is Send (multi-thread safe)
// ---------------------------------------------------------------------------

#[cfg(feature = "send")]
mod send_feature {
    use super::*;

    fn assert_send<T: Send>() {}

    #[test]
    fn lua_is_send() {
        assert_send::<Lua>();
    }

    #[test]
    fn bound_lua_is_send() {
        let lua = bound_lua();
        // Move to another thread and use it.
        let handle = std::thread::spawn(move || {
            let val: i64 = lua.load("return geo.COUNT").eval().unwrap();
            val
        });
        assert_eq!(handle.join().unwrap(), 42);
    }
}
