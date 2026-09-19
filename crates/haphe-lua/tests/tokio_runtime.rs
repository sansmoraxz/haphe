//! Validates that binding and using a bound Lua state works under both
//! single-threaded and multi-threaded tokio runtimes.
//!
//! - Single-thread tests always run (mlua's `Lua` is always usable on a
//!   single thread).
//! - Multi-thread tests are gated on `feature = "send"` — without it `Lua`
//!   is `!Send` and cannot cross a `tokio::spawn` boundary.

#![allow(
    dead_code,
    reason = "fixtures are exercised through their generated descriptors and bridge wrappers, not direct calls"
)]

use haphe::{RuntimeBinder, Script, script};
use haphe_lua::LuaBinder;
use mlua::Lua;

// ---------------------------------------------------------------------------
// Shared domain — same types an external user would define
// ---------------------------------------------------------------------------

#[derive(Script, Clone)]
#[script(thread_safety = send_sync, traits(Display, PartialEq), methods)]
struct Vec2 {
    x: f64,
    y: f64,
}

impl std::fmt::Display for Vec2 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {})", self.x, self.y)
    }
}

impl PartialEq for Vec2 {
    fn eq(&self, other: &Self) -> bool {
        self.x == other.x && self.y == other.y
    }
}

#[script]
impl Vec2 {
    #[script(constructor)]
    fn new(x: f64, y: f64) -> Self {
        Vec2 { x, y }
    }
}

#[script]
fn double(n: i32) -> i32 {
    n * 2
}

haphe::registry! {
    pub static REGISTRY = {
        structs: [Vec2],
        modules: [
            mod vectors {
                functions: [double],
                types: [Vec2],
                constants: [
                    ORIGIN_X: f64 = 0.0,
                    ORIGIN_Y: f64 = 0.0,
                    DIMENSIONS: i32 = 2,
                ],
            },
        ],
    };
}

fn bind_fresh() -> Lua {
    let mut lua = Lua::new();
    let binder = LuaBinder::new();
    let validated = REGISTRY.validate().unwrap();
    binder.bind(&validated, &mut lua).unwrap();
    lua
}

// ---------------------------------------------------------------------------
// Single-threaded runtime (always available)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn single_thread_bind_and_read_constants() {
    let lua = bind_fresh();

    let dims: i64 = lua.load("return vectors.DIMENSIONS").eval().unwrap();
    assert_eq!(dims, 2);
}

#[tokio::test(flavor = "current_thread")]
async fn single_thread_multiple_eval_rounds() {
    let lua = bind_fresh();

    for i in 0..10 {
        let result: f64 = lua
            .load(format!("return vectors.ORIGIN_X + {i}"))
            .eval()
            .unwrap();
        assert!((result - f64::from(i)).abs() < 1e-12);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn single_thread_spawn_local() {
    let lua = bind_fresh();

    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(async {
            tokio::task::spawn_local(async move {
                let val: i64 = lua.load("return vectors.DIMENSIONS").eval().unwrap();
                val
            })
            .await
            .unwrap()
        })
        .await;

    assert_eq!(result, 2);
}

#[tokio::test(flavor = "current_thread")]
async fn single_thread_concurrent_local_tasks() {
    let local = tokio::task::LocalSet::new();

    local
        .run_until(async {
            let mut handles = Vec::new();
            for _ in 0..4 {
                handles.push(tokio::task::spawn_local(async {
                    let lua = bind_fresh();
                    let pi: f64 = lua.load("return vectors.ORIGIN_X").eval().unwrap();
                    pi
                }));
            }
            for handle in handles {
                let val = handle.await.unwrap();
                assert!((val - 0.0).abs() < 1e-12);
            }
        })
        .await;
}

// ---------------------------------------------------------------------------
// Multi-threaded runtime (requires `send` feature → mlua/send → Lua: Send)
// ---------------------------------------------------------------------------

#[cfg(feature = "send")]
mod multi_thread {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_bind_and_eval() {
        let handle = tokio::spawn(async {
            let lua = bind_fresh();
            let dims: i64 = lua.load("return vectors.DIMENSIONS").eval().unwrap();
            dims
        });

        assert_eq!(handle.await.unwrap(), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn parallel_independent_lua_states() {
        let mut handles = Vec::new();

        for i in 0..8 {
            handles.push(tokio::spawn(async move {
                let lua = bind_fresh();
                let result: f64 = lua
                    .load(format!("return vectors.ORIGIN_X + {i}"))
                    .eval()
                    .unwrap();
                (i, result)
            }));
        }

        for handle in handles {
            let (i, result) = handle.await.unwrap();
            assert!(
                (result - i as f64).abs() < 1e-12,
                "task {i}: expected {i}, got {result}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn lua_state_moves_across_threads() {
        // Bind on one task, hand the Lua state to another.
        let (tx, rx) = tokio::sync::oneshot::channel::<Lua>();

        let producer = tokio::spawn(async {
            let lua = bind_fresh();
            tx.send(lua).unwrap();
        });

        let consumer = tokio::spawn(async {
            let lua = rx.await.unwrap();
            let dims: i64 = lua.load("return vectors.DIMENSIONS").eval().unwrap();
            dims
        });

        producer.await.unwrap();
        assert_eq!(consumer.await.unwrap(), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn function_stub_errors_from_spawned_task() {
        let handle = tokio::spawn(async {
            let lua = bind_fresh();
            let err = lua
                .load("vectors.double(5)")
                .exec()
                .expect_err("stub should error");
            err.to_string()
        });

        let msg = handle.await.unwrap();
        assert!(msg.contains("not yet implemented"), "got: {msg}");
    }
}
