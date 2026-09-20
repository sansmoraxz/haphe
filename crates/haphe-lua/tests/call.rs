//! Callable userdata: `traits(Call)` / `traits(AsyncCall)` drive Lua's
//! `__call` metamethod (sync everywhere; async behind the `async` feature).

#![allow(
    dead_code,
    reason = "fixtures are exercised through their generated descriptors and bridge wrappers, not direct calls"
)]

use haphe::Script;
use haphe_lua::bind_type;
use mlua::Lua;

#[derive(Script, Clone)]
#[script(traits(Call(args = (i64, i64), output = i64)))]
struct Adder {
    base: i64,
}

impl haphe::ops::Call<(i64, i64)> for Adder {
    type Output = i64;
    fn call(&self, (a, b): (i64, i64)) -> i64 {
        self.base + a + b
    }
}

#[derive(Script, Clone)]
#[script(traits(Call(args = (), output = String)))]
struct Ping {
    word: String,
}

impl haphe::ops::Call<()> for Ping {
    type Output = String;
    fn call(&self, (): ()) -> String {
        self.word.clone()
    }
}

#[test]
fn sync_call_works() {
    let lua = Lua::new();
    let tbl = lua.create_table().unwrap();
    bind_type::<Adder>(&lua, &tbl).unwrap();
    lua.globals()
        .set("add", lua.create_any_userdata(Adder { base: 100 }).unwrap())
        .unwrap();
    let out: i64 = lua.load("return add(2, 3)").eval().unwrap();
    assert_eq!(out, 105);
}

#[test]
fn zero_arg_call_works() {
    let lua = Lua::new();
    let tbl = lua.create_table().unwrap();
    bind_type::<Ping>(&lua, &tbl).unwrap();
    lua.globals()
        .set(
            "ping",
            lua.create_any_userdata(Ping {
                word: "pong".into(),
            })
            .unwrap(),
        )
        .unwrap();
    let out: String = lua.load("return ping()").eval().unwrap();
    assert_eq!(out, "pong");
}

#[test]
fn wrong_arg_type_errors() {
    let lua = Lua::new();
    let tbl = lua.create_table().unwrap();
    bind_type::<Adder>(&lua, &tbl).unwrap();
    lua.globals()
        .set("add", lua.create_any_userdata(Adder { base: 0 }).unwrap())
        .unwrap();
    assert!(
        lua.load("return add('two', 3)")
            .eval::<mlua::Value>()
            .is_err()
    );
}

// ---------------------------------------------------------------------------
// Async call
// ---------------------------------------------------------------------------

#[derive(Script, Clone)]
#[script(thread_safety = none, traits(AsyncCall(args = (i64,), output = i64)))]
struct SlowDoubler {
    factor: i64,
}

impl haphe::ops::AsyncCall<(i64,)> for SlowDoubler {
    type Output = i64;
    async fn call_async(&self, (n,): (i64,)) -> i64 {
        tokio::task::yield_now().await;
        self.factor * n
    }
}

#[cfg(all(
    feature = "async",
    not(feature = "send"),
    not(any(feature = "lua51", feature = "luau"))
))]
#[tokio::test]
async fn async_call_works() {
    let lua = Lua::new();
    let tbl = lua.create_table().unwrap();
    bind_type::<SlowDoubler>(&lua, &tbl).unwrap();
    lua.globals()
        .set(
            "double",
            lua.create_any_userdata(SlowDoubler { factor: 2 }).unwrap(),
        )
        .unwrap();
    let out: i64 = lua.load("return double(21)").eval_async().await.unwrap();
    assert_eq!(out, 42);
}

#[cfg(not(feature = "async"))]
#[test]
fn async_call_rejected_without_async_feature() {
    let lua = Lua::new();
    let tbl = lua.create_table().unwrap();
    let Err(err) = bind_type::<SlowDoubler>(&lua, &tbl) else {
        panic!("AsyncCall must be rejected without the `async` feature");
    };
    let haphe_lua::LuaBindError::UnsupportedAsyncCall { .. } = err else {
        panic!("expected UnsupportedAsyncCall, got {err}");
    };
    assert!(err.to_string().contains("async"));
}

#[cfg(all(
    feature = "async",
    not(feature = "send"),
    not(any(feature = "lua51", feature = "luau"))
))]
mod ambiguous {
    use super::*;

    #[derive(Script, Clone)]
    #[script(thread_safety = none, traits(
        Call(args = (i64,), output = i64),
        AsyncCall(args = (i64,), output = i64),
    ))]
    struct BothWays {
        n: i64,
    }

    impl haphe::ops::Call<(i64,)> for BothWays {
        type Output = i64;
        fn call(&self, (x,): (i64,)) -> i64 {
            self.n + x
        }
    }

    impl haphe::ops::AsyncCall<(i64,)> for BothWays {
        type Output = i64;
        async fn call_async(&self, (x,): (i64,)) -> i64 {
            self.n + x
        }
    }

    #[test]
    fn both_call_traits_rejected_as_ambiguous() {
        let lua = Lua::new();
        let tbl = lua.create_table().unwrap();
        let Err(err) = bind_type::<BothWays>(&lua, &tbl) else {
            panic!("declaring Call and AsyncCall together must be rejected");
        };
        assert!(matches!(err, haphe_lua::LuaBindError::AmbiguousCall));
        assert!(err.to_string().contains("__call"));
    }
}
