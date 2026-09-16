//! Async `#[script] impl` methods bridged onto Lua userdata (mlua's async
//! methods exist on every Lua version, behind this backend's `async`
//! feature).

#![allow(dead_code)]

use haphe::{Script, script};
use haphe_lua::bind_type;
use mlua::Lua;

#[derive(Script, Clone)]
#[script(thread_safety = none, methods)]
struct Fetcher {
    prefix: String,
    hits: i64,
}

#[script]
impl Fetcher {
    #[script(constructor)]
    fn new(prefix: String) -> Self {
        Fetcher { prefix, hits: 0 }
    }

    async fn fetch(&self, path: String) -> String {
        tokio::task::yield_now().await;
        format!("{}/{}", self.prefix, path)
    }

    async fn bump(&mut self, by: i64) -> i64 {
        tokio::task::yield_now().await;
        self.hits += by + 1;
        self.hits
    }

    fn hit_count(&self) -> i64 {
        self.hits
    }

    async fn touch(&self) {
        tokio::task::yield_now().await;
    }
}

#[cfg(all(feature = "async", not(feature = "send")))]
mod with_async {
    use super::*;

    fn env(lua: &Lua) {
        let tbl = lua.create_table().unwrap();
        bind_type::<Fetcher>(lua, &tbl).unwrap();
        lua.globals().set("Fetcher", &tbl).unwrap();
        lua.load("f = Fetcher.new('api')").exec().unwrap();
    }

    #[tokio::test]
    async fn ref_async_method_returns_value() {
        let lua = Lua::new();
        env(&lua);
        let out: String = lua
            .load("return f:fetch('users')")
            .eval_async()
            .await
            .unwrap();
        assert_eq!(out, "api/users");
    }

    #[tokio::test]
    async fn no_return_async_method_yields_nil() {
        let lua = Lua::new();
        env(&lua);
        let out: mlua::Value = lua.load("return f:touch()").eval_async().await.unwrap();
        assert!(out.is_nil());
    }

    #[tokio::test]
    async fn wrong_arg_type_errors() {
        let lua = Lua::new();
        env(&lua);
        assert!(
            lua.load("return f:fetch({})")
                .eval_async::<mlua::Value>()
                .await
                .is_err()
        );
    }

    // `&mut self` async methods hold the mutable guard across awaits, so
    // the future mutates the bound userdata IN PLACE — script-visible
    // mutation persists.
    #[tokio::test]
    async fn mut_async_method_mutates_in_place() {
        let lua = Lua::new();
        env(&lua);
        let bumped: i64 = lua.load("return f:bump(9)").eval_async().await.unwrap();
        assert_eq!(bumped, 10);
        let hits: i64 = lua.load("return f:hit_count()").eval_async().await.unwrap();
        assert_eq!(hits, 10, "mutation persisted on the same userdata");
    }
}

#[cfg(all(feature = "async", feature = "send"))]
#[test]
fn async_method_rejected_under_send() {
    let lua = Lua::new();
    let tbl = lua.create_table().unwrap();
    let Err(err) = bind_type::<Fetcher>(&lua, &tbl) else {
        panic!("async methods must be rejected under the `send` feature");
    };
    let haphe_lua::LuaBindError::UnsupportedAsyncMethod { .. } = err else {
        panic!("expected UnsupportedAsyncMethod, got {err}");
    };
    assert!(err.to_string().contains("Send"));
}

#[cfg(not(feature = "async"))]
#[test]
fn async_method_rejected_without_async_feature() {
    let lua = Lua::new();
    let tbl = lua.create_table().unwrap();
    let Err(err) = bind_type::<Fetcher>(&lua, &tbl) else {
        panic!("async methods must be rejected without the `async` feature");
    };
    let haphe_lua::LuaBindError::UnsupportedAsyncMethod { name, .. } = err else {
        panic!("expected UnsupportedAsyncMethod, got {err}");
    };
    assert!(name == "fetch" || name == "bump" || name == "touch");
    assert!(err.to_string().contains(name));
}
