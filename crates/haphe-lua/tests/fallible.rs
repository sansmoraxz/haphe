//! Fallible surfaces through the VM: `Result<T, E>` constructors, methods,
//! and free functions bind; `Ok` crosses as the value, `Err` raises a Lua
//! error carrying the rendered Host message, prefixed by the declared
//! `error_kind` ("`ValueError`: ..."). Conversion errors keep their existing
//! text.

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
use haphe_lua::{bind_fn, bind_type};
use mlua::Lua;

#[derive(Script, Clone, Debug)]
#[script(thread_safety = none, methods)]
struct Meter {
    level: i64,
}

#[script]
impl Meter {
    #[script(constructor, error_kind = "ValueError")]
    fn new(level: i64) -> Result<Self, String> {
        if level < 0 {
            Err(format!("negative level {level}"))
        } else {
            Ok(Meter { level })
        }
    }

    #[script(error_kind = "RangeError")]
    fn checked_add(&self, amount: i64) -> Result<i64, String> {
        self.level
            .checked_add(amount)
            .ok_or_else(|| "overflow".to_string())
    }

    fn drain(&mut self, amount: i64) -> Result<(), String> {
        if amount > self.level {
            return Err("insufficient".to_string());
        }
        self.level -= amount;
        Ok(())
    }

    fn remaining(&self) -> i64 {
        self.level
    }
}

/// Parses a meter level.
#[script(error_kind = "ParseError")]
fn parse_level(raw: String) -> Result<i64, std::num::ParseIntError> {
    raw.parse()
}

fn env() -> Lua {
    let lua = Lua::new();
    let tbl = lua.create_table().unwrap();
    bind_type::<Meter>(&lua, &tbl).unwrap();
    lua.globals().set("Meter", tbl).unwrap();
    let fns = lua.create_table().unwrap();
    bind_fn::<parse_level>(&lua, &fns).unwrap();
    lua.globals().set("fns", fns).unwrap();
    lua
}

#[test]
fn fallible_constructor_ok_and_err() {
    let lua = env();
    let level: i64 = lua
        .load("local m = Meter.new(4) return m:remaining()")
        .eval()
        .unwrap();
    assert_eq!(level, 4);
    let (ok, err): (bool, String) = lua
        .load("local ok, e = pcall(Meter.new, -1) return ok, tostring(e)")
        .eval()
        .unwrap();
    assert!(!ok);
    assert!(
        err.contains("ValueError: negative level -1"),
        "kind-prefixed message, got {err:?}"
    );
}

#[test]
fn fallible_method_ok_and_err() {
    let lua = env();
    let sum: i64 = lua
        .load("local m = Meter.new(40) return m:checked_add(2)")
        .eval()
        .unwrap();
    assert_eq!(sum, 42);
    let err: String = lua
        .load(
            "local m = Meter.new(1) \
             local ok, e = pcall(function() return m:checked_add(9223372036854775807) end) \
             assert(not ok) return tostring(e)",
        )
        .eval()
        .unwrap();
    assert!(err.contains("RangeError: overflow"), "got {err:?}");
}

#[test]
fn fallible_unit_method_mutates_or_raises() {
    let lua = env();
    let left: i64 = lua
        .load("local m = Meter.new(10) m:drain(4) return m:remaining()")
        .eval()
        .unwrap();
    assert_eq!(left, 6, "Ok(()) crosses as nil and mutation persists");
    let err: String = lua
        .load(
            "local m = Meter.new(1) \
             local ok, e = pcall(function() m:drain(100) end) \
             assert(not ok) return tostring(e)",
        )
        .eval()
        .unwrap();
    // No declared kind: the bare message, no prefix.
    assert!(err.contains("insufficient"), "got {err:?}");
    assert!(
        !err.contains("insufficient:"),
        "no stray prefix, got {err:?}"
    );
}

#[test]
fn fallible_free_fn_ok_and_err() {
    let lua = env();
    let n: i64 = lua.load("return fns.parse_level('42')").eval().unwrap();
    assert_eq!(n, 42);
    let err: String = lua
        .load("local ok, e = pcall(fns.parse_level, 'nope') assert(not ok) return tostring(e)")
        .eval()
        .unwrap();
    assert!(err.contains("ParseError:"), "got {err:?}");
}

#[test]
fn conversion_errors_keep_their_text() {
    let lua = env();
    let err: String = lua
        .load("local ok, e = pcall(Meter.new, {}) assert(not ok) return tostring(e)")
        .eval()
        .unwrap();
    assert!(
        err.contains("expected") && err.contains("got"),
        "convert error text unchanged, got {err:?}"
    );
}

#[cfg(all(feature = "async", not(feature = "send")))]
mod with_async {
    use super::*;

    #[derive(Script, Clone)]
    #[script(thread_safety = none, methods)]
    struct Probe {
        level: i64,
    }

    #[script]
    impl Probe {
        #[script(constructor)]
        fn new(level: i64) -> Self {
            Probe { level }
        }

        #[script(error_kind = "IOError")]
        async fn refresh(&self) -> Result<i64, String> {
            tokio::task::yield_now().await;
            if self.level == 0 {
                Err("empty".to_string())
            } else {
                Ok(self.level)
            }
        }
    }

    fn async_env() -> Lua {
        let lua = Lua::new();
        let tbl = lua.create_table().unwrap();
        bind_type::<Probe>(&lua, &tbl).unwrap();
        lua.globals().set("Probe", tbl).unwrap();
        lua
    }

    #[tokio::test]
    async fn fallible_async_method_ok_and_err() {
        let lua = async_env();
        let level: i64 = lua
            .load("local p = Probe.new(5) return p:refresh()")
            .eval_async()
            .await
            .unwrap();
        assert_eq!(level, 5);
        let err = lua
            .load("local p = Probe.new(0) return p:refresh()")
            .eval_async::<i64>()
            .await
            .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("IOError: empty"), "got {text:?}");
    }
}
