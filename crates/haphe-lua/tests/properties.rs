//! Computed properties (`#[script(getter)]`/`#[script(setter)]`) bound as
//! Lua field accessors, and async constructors on the type table.

#![allow(
    dead_code,
    reason = "fixtures are exercised through their generated descriptors and bridge wrappers, not direct calls"
)]

use haphe::{Script, script};
use haphe_lua::bind_type;
use mlua::Lua;

#[derive(Script, Clone)]
#[script(thread_safety = none, methods)]
struct Gauge {
    #[script(skip)]
    raw: i64,
}

#[script]
impl Gauge {
    #[script(constructor)]
    fn new(raw: i64) -> Self {
        Gauge { raw }
    }

    /// The doubled reading.
    #[script(getter)]
    fn level(&self) -> i64 {
        self.raw * 2
    }

    #[script(setter)]
    fn set_level(&mut self, value: i64) {
        self.raw = value / 2;
    }

    /// Read-only view of the raw value.
    #[script(getter)]
    fn raw_value(&self) -> i64 {
        self.raw
    }
}

fn env(lua: &Lua) {
    let tbl = lua.create_table().unwrap();
    bind_type::<Gauge>(lua, &tbl).unwrap();
    lua.globals().set("Gauge", tbl).unwrap();
}

#[test]
fn property_reads_and_writes_as_field() {
    let lua = Lua::new();
    env(&lua);
    let out: i64 = lua
        .load("local g = Gauge.new(21); return g.level")
        .eval()
        .unwrap();
    assert_eq!(out, 42);
    let out: i64 = lua
        .load("local g = Gauge.new(0); g.level = 10; return g.raw_value")
        .eval()
        .unwrap();
    assert_eq!(out, 5);
}

#[test]
fn readonly_property_has_no_setter() {
    let lua = Lua::new();
    env(&lua);
    let err = lua
        .load("local g = Gauge.new(1); g.raw_value = 9")
        .exec()
        .expect_err("no setter registered");
    let msg = err.to_string();
    assert!(!msg.is_empty(), "got: {msg}");
}

#[test]
fn property_set_conversion_failure_errors() {
    let lua = Lua::new();
    env(&lua);
    let err = lua
        .load("local g = Gauge.new(1); g.level = 'nope'")
        .exec()
        .expect_err("string does not convert to i64");
    assert!(err.to_string().contains("expected"), "got: {err}");
}

#[cfg(all(feature = "async", not(feature = "send")))]
mod async_ctor {
    use super::*;

    #[derive(Script, Clone)]
    #[script(thread_safety = none, methods)]
    struct Probe {
        #[script(skip)]
        id: i64,
    }

    #[script]
    impl Probe {
        #[script(constructor)]
        async fn new(id: i64) -> Self {
            Probe { id }
        }

        #[script(getter)]
        fn id(&self) -> i64 {
            self.id
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn async_constructor_builds_userdata() {
        let lua = Lua::new();
        let tbl = lua.create_table().unwrap();
        bind_type::<Probe>(&lua, &tbl).unwrap();
        lua.globals().set("Probe", tbl).unwrap();
        let out: i64 = lua
            .load("local p = Probe.new(9); return p.id")
            .eval_async()
            .await
            .unwrap();
        assert_eq!(out, 9);
    }
}

#[cfg(not(feature = "async"))]
mod no_async_feature {
    use super::*;

    #[derive(Script, Clone)]
    #[script(thread_safety = none, methods)]
    struct Probe {
        #[script(skip)]
        id: i64,
    }

    #[script]
    impl Probe {
        #[script(constructor)]
        async fn new(id: i64) -> Self {
            Probe { id }
        }
    }

    #[test]
    fn async_constructor_rejected_without_feature() {
        let lua = Lua::new();
        let tbl = lua.create_table().unwrap();
        let Err(err) = bind_type::<Probe>(&lua, &tbl) else {
            panic!("expected rejection");
        };
        let msg = err.to_string();
        assert!(
            msg.contains("new") && msg.contains("async"),
            "got: {msg}"
        );
    }
}

#[cfg(all(feature = "async", not(feature = "send")))]
mod async_property_policy {
    use super::*;

    #[derive(Script, Clone)]
    #[script(thread_safety = send_sync, methods)]
    struct Sensor {
        #[script(skip)]
        raw: i64,
    }

    #[script]
    impl Sensor {
        #[script(getter)]
        async fn reading(&self) -> i64 {
            self.raw
        }
    }

    #[test]
    fn async_property_rejected_descriptively() {
        // mlua exposes no async field accessors; the registration must say
        // so rather than silently dropping or inventing method names.
        let lua = Lua::new();
        let tbl = lua.create_table().unwrap();
        let Err(err) = bind_type::<Sensor>(&lua, &tbl) else {
            panic!("expected rejection");
        };
        let msg = err.to_string();
        assert!(
            msg.contains("reading") && msg.contains("async field"),
            "got: {msg}"
        );
    }
}
