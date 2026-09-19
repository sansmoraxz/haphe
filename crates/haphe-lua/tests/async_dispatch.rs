//! Async trait-presence dispatch end to end: transparent-primitive-newtype
//! async methods and free fns register through the async channels and cross
//! as native Lua values.

#![allow(
    dead_code,
    reason = "fixtures are exercised through their generated descriptors and bridge wrappers, not direct calls"
)]

use haphe::{Script, script};

/// A transparent bool newtype: crosses as a native boolean.
#[derive(Script, Clone, Copy)]
#[script(transparent)]
struct Flag(bool);

#[derive(Script, Clone)]
#[script(thread_safety = send_sync, methods)]
struct Toggle {
    powered: Flag,
}

#[script]
impl Toggle {
    #[script(constructor)]
    fn new(on: bool) -> Self {
        Toggle { powered: Flag(on) }
    }

    async fn toggled_later(&self, next: Flag) -> Flag {
        tokio::task::yield_now().await;
        Flag(self.powered.0 != next.0)
    }

    async fn set_later(&mut self, next: Flag) {
        tokio::task::yield_now().await;
        self.powered = next;
    }
}

/// Newtype-typed async free fn: dispatch-registered via `function_async`.
#[script]
async fn invert_later(flag: Flag) -> Flag {
    tokio::task::yield_now().await;
    Flag(!flag.0)
}

#[cfg(all(feature = "async", not(feature = "send")))]
mod with_async {
    use super::*;
    use haphe_lua::{bind_fn, bind_type};
    use mlua::Lua;

    fn env(lua: &Lua) {
        let ty = lua.create_table().unwrap();
        bind_type::<Toggle>(lua, &ty).unwrap();
        lua.globals().set("Toggle", &ty).unwrap();
        let m = lua.create_table().unwrap();
        bind_fn::<invert_later>(lua, &m).unwrap();
        lua.globals().set("m", &m).unwrap();
        lua.load("t = Toggle.new(true)").exec().unwrap();
    }

    #[tokio::test]
    async fn newtype_async_method_crosses_as_boolean() {
        let lua = Lua::new();
        env(&lua);
        let (out, is_bool): (bool, bool) = lua
            .load("local v = t:toggled_later(true) return v, type(v) == 'boolean'")
            .eval_async()
            .await
            .unwrap();
        assert!(!out, "true XOR true");
        assert!(is_bool, "crosses as a native boolean");
    }

    // `&mut self` async dispatch holds the mutable guard: mutation is
    // script-visible afterwards.
    #[tokio::test]
    async fn newtype_mut_async_method_mutates_in_place() {
        let lua = Lua::new();
        env(&lua);
        lua.load("t:set_later(false)").exec_async().await.unwrap();
        let powered: bool = lua.load("return t.powered").eval_async().await.unwrap();
        assert!(!powered, "mutation persisted on the same userdata");
    }

    #[tokio::test]
    async fn newtype_async_free_fn_dispatches() {
        let lua = Lua::new();
        env(&lua);
        let out: bool = lua
            .load("return m.invert_later(false)")
            .eval_async()
            .await
            .unwrap();
        assert!(out);
    }

    #[tokio::test]
    async fn newtype_async_method_wrong_arg_errors() {
        let lua = Lua::new();
        env(&lua);
        assert!(
            lua.load("return t:toggled_later({})")
                .eval_async::<mlua::Value>()
                .await
                .is_err()
        );
    }
}
