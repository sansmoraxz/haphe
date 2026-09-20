//! Fallible surfaces through the VM: `Result<T, E>` constructors, methods,
//! and free functions bind; `Ok` crosses as the value, `Err` raises a Lua
//! error carrying the rendered callee message, prefixed by the declared
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
    fn new(level: i64) -> Result<Self, TextError> {
        if level < 0 {
            Err(TextError(format!("negative level {level}")))
        } else {
            Ok(Meter { level })
        }
    }

    #[script(error_kind = "RangeError")]
    fn checked_add(&self, amount: i64) -> Result<i64, TextError> {
        self.level
            .checked_add(amount)
            .ok_or_else(|| TextError("overflow".into()))
    }

    fn drain(&mut self, amount: i64) -> Result<(), TextError> {
        if amount > self.level {
            return Err(TextError("insufficient".into()));
        }
        self.level -= amount;
        Ok(())
    }

    #[script(error_kind = "IoError")]
    fn reload(&self) -> Result<i64, Layered> {
        Err(Layered { source: RootCause })
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
    haphe_lua::install_error_info(&lua).unwrap();
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
        async fn refresh(&self) -> Result<i64, TextError> {
            tokio::task::yield_now().await;
            if self.level == 0 {
                Err(TextError("empty".into()))
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

/// A two-level error: `source()` renders into the decoded `chain`.
#[derive(Debug)]
struct RootCause;

impl std::fmt::Display for RootCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("root cause")
    }
}

impl std::error::Error for RootCause {}

#[derive(Debug)]
struct Layered {
    source: RootCause,
}

impl std::fmt::Display for Layered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("layered failure")
    }
}

impl std::error::Error for Layered {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

// ---------------------------------------------------------------------------
// Structured error access: haphe_error(e)
// ---------------------------------------------------------------------------

#[test]
fn host_error_decodes_to_structured_table() {
    let lua = env();
    let (kind, message, type_name, chain_len): (String, String, String, i64) = lua
        .load(
            "local m = Meter.new(1) \
             local ok, e = pcall(function() return m:checked_add(9223372036854775807) end) \
             assert(not ok) \
             local info = haphe_error(e) \
             return info.kind, info.message, info.type, #info.chain",
        )
        .eval()
        .unwrap();
    assert_eq!(kind, "RangeError");
    assert_eq!(message, "overflow");
    assert!(type_name.contains("TextError"), "got {type_name:?}");
    assert_eq!(chain_len, 0, "TextError has no source chain");
}

#[test]
fn source_chain_and_type_cross_into_the_decoded_table() {
    let lua = env();
    let (kind, message, type_name, first_cause, rendered): (
        String,
        String,
        String,
        String,
        String,
    ) = lua
        .load(
            "local m = Meter.new(1) \
             local ok, e = pcall(function() return m:reload() end) \
             assert(not ok) \
             local info = haphe_error(e) \
             return info.kind, info.message, info.type, info.chain[1], tostring(e)",
        )
        .eval()
        .unwrap();
    assert_eq!(kind, "IoError");
    assert_eq!(message, "layered failure");
    assert!(type_name.contains("Layered"), "got {type_name:?}");
    assert_eq!(first_cause, "root cause");
    // The plain string rendering is unchanged by the structured surface.
    assert!(
        rendered.contains("IoError: layered failure"),
        "got {rendered:?}"
    );
}

#[test]
fn kindless_host_error_decodes_without_kind() {
    let lua = env();
    let (kind_is_nil, message): (bool, String) = lua
        .load(
            "local m = Meter.new(1) \
             local ok, e = pcall(function() m:drain(100) end) \
             assert(not ok) \
             local info = haphe_error(e) \
             return info.kind == nil, info.message",
        )
        .eval()
        .unwrap();
    assert!(kind_is_nil);
    assert_eq!(message, "insufficient");
}

#[test]
fn non_host_values_decode_to_nil() {
    let lua = env();
    let (plain, convert): (bool, bool) = lua
        .load(
            "local plain = haphe_error('just a string') == nil \
             local ok, e = pcall(function() local m = Meter.new(1) return m:checked_add('x') end) \
             assert(not ok) \
             return plain, haphe_error(e) == nil",
        )
        .eval()
        .unwrap();
    assert!(plain, "a plain string error is not a callee error");
    assert!(convert, "a conversion error is not a callee error");
}
