//! Async free functions bridged through `FnBinder::function_async` (mlua's
//! async functions, behind this backend's `async` feature).

#![allow(dead_code)]

use haphe::script;
use haphe_lua::bind_fn;
use mlua::Lua;

/// Sums two integers after a yield.
#[script]
async fn delayed_sum(a: i64, b: i64) -> i64 {
    #[cfg(feature = "async")]
    tokio::task::yield_now().await;
    a + b
}

#[cfg(all(feature = "async", not(feature = "send")))]
mod with_async {
    use super::*;

    fn env(lua: &Lua) {
        let tbl = lua.create_table().unwrap();
        bind_fn::<delayed_sum>(lua, &tbl).unwrap();
        lua.globals().set("m", &tbl).unwrap();
    }

    #[tokio::test]
    async fn async_free_fn_returns_awaited_value() {
        let lua = Lua::new();
        env(&lua);
        let out: i64 = lua
            .load("return m.delayed_sum(20, 22)")
            .eval_async()
            .await
            .unwrap();
        assert_eq!(out, 42);
    }

    #[tokio::test]
    async fn async_free_fn_wrong_arg_errors() {
        let lua = Lua::new();
        env(&lua);
        let err = lua
            .load("return m.delayed_sum('x', 2)")
            .eval_async::<i64>()
            .await
            .expect_err("string is not an integer");
        assert!(err.to_string().contains("expected"), "got: {err}");
    }
}

#[cfg(not(feature = "async"))]
#[test]
fn async_free_fn_rejected_without_async_feature() {
    let lua = Lua::new();
    let tbl = lua.create_table().unwrap();
    let err = bind_fn::<delayed_sum>(&lua, &tbl).expect_err("async needs the feature");
    let msg = err.to_string();
    assert!(
        msg.contains("delayed_sum") && msg.contains("async"),
        "got: {msg}"
    );
}
