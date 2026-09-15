//! Tests that `#[derive(Script)]` + `#[script] impl` automatically produces
//! working Lua UserData — no hand-written mlua code required.

#![allow(dead_code)]

use haphe::{RuntimeBinder, Script, script};
use haphe_lua::{LuaBinder, bind_fn, bind_type};
use mlua::Lua;

// ---------------------------------------------------------------------------
// Domain types — ONLY haphe attributes, no mlua imports
// ---------------------------------------------------------------------------

#[derive(Script, Clone)]
#[script(
    thread_safety = send_sync,
    traits(
        Display, PartialEq,
        Add, Add(rhs = f64, output = Self),
        Mul(rhs = f64, output = Self),
        Div(rhs = f64, output = Self),
    ),
    methods,
)]
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

impl std::ops::Add for Vec2 {
    type Output = Vec2;
    fn add(self, rhs: Vec2) -> Vec2 {
        Vec2 {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
        }
    }
}

impl std::ops::Add<f64> for Vec2 {
    type Output = Vec2;
    fn add(self, rhs: f64) -> Vec2 {
        Vec2 {
            x: self.x + rhs,
            y: self.y + rhs,
        }
    }
}

impl std::ops::Mul<f64> for Vec2 {
    type Output = Vec2;
    fn mul(self, rhs: f64) -> Vec2 {
        Vec2 {
            x: self.x * rhs,
            y: self.y * rhs,
        }
    }
}

impl std::ops::Div<f64> for Vec2 {
    type Output = Vec2;
    fn div(self, rhs: f64) -> Vec2 {
        Vec2 {
            x: self.x / rhs,
            y: self.y / rhs,
        }
    }
}

#[script]
impl Vec2 {
    #[script(constructor)]
    fn new(x: f64, y: f64) -> Self {
        Vec2 { x, y }
    }

    fn length(&self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
}

/// Adds two integers.
#[script]
fn add(a: i32, b: i32) -> i32 {
    a + b
}

/// Multiplies two floats.
#[script]
fn mul(a: f64, b: f64) -> f64 {
    a * b
}

/// Greets someone.
#[script]
fn greet(name: String) -> String {
    format!("hello, {name}!")
}

haphe::registry! {
    pub static REGISTRY = {
        structs: [Vec2],
        modules: [
            mod vectors {
                functions: [add, mul, greet],
                types: [Vec2],
            },
        ],
    };
}

fn setup_lua() -> Lua {
    let mut lua = Lua::new();
    let binder = LuaBinder::new();
    let validated = REGISTRY.validate().unwrap();
    binder.bind(&validated, &mut lua).unwrap();

    let globals = lua.globals();
    let vectors: mlua::Table = globals.get("vectors").unwrap();

    // Register Vec2's bridge bindings.
    let vec2_table: mlua::Table = vectors.get("Vec2").unwrap();
    bind_type::<Vec2>(&lua, &vec2_table).unwrap();

    // Register free functions (replaces the stubs).
    bind_fn::<add>(&lua, &vectors).unwrap();
    bind_fn::<mul>(&lua, &vectors).unwrap();
    bind_fn::<greet>(&lua, &vectors).unwrap();

    lua
}

// ---------------------------------------------------------------------------
// Tests — all Lua-side, no mlua UserData code in this file
// ---------------------------------------------------------------------------

#[test]
fn constructor_works() {
    let lua = setup_lua();
    let result: bool = lua
        .load("return vectors.Vec2.new(1, 2) ~= nil")
        .eval()
        .unwrap();
    assert!(result);
}

#[test]
fn field_read() {
    let lua = setup_lua();
    let x: f64 = lua.load("return vectors.Vec2.new(3, 4).x").eval().unwrap();
    assert!((x - 3.0).abs() < 1e-12);
}

#[test]
fn field_write() {
    let lua = setup_lua();
    let result: f64 = lua
        .load(
            r#"
            local v = vectors.Vec2.new(1, 2)
            v.x = 99
            return v.x
            "#,
        )
        .eval()
        .unwrap();
    assert!((result - 99.0).abs() < 1e-12);
}

#[test]
fn method_call() {
    let lua = setup_lua();
    let len: f64 = lua
        .load("return vectors.Vec2.new(3, 4):length()")
        .eval()
        .unwrap();
    assert!((len - 5.0).abs() < 1e-12);
}

#[test]
fn tostring_metamethod() {
    let lua = setup_lua();
    let s: String = lua
        .load("return tostring(vectors.Vec2.new(1, 2))")
        .eval()
        .unwrap();
    assert_eq!(s, "(1, 2)");
}

#[test]
fn eq_metamethod() {
    let lua = setup_lua();
    let eq: bool = lua
        .load("return vectors.Vec2.new(1, 2) == vectors.Vec2.new(1, 2)")
        .eval()
        .unwrap();
    assert!(eq);
}

#[test]
fn add_metamethod() {
    let lua = setup_lua();
    let x: f64 = lua
        .load("return (vectors.Vec2.new(1, 2) + vectors.Vec2.new(3, 4)).x")
        .eval()
        .unwrap();
    assert!((x - 4.0).abs() < 1e-12);
}

#[test]
fn mul_scalar_right() {
    let lua = setup_lua();
    let x: f64 = lua
        .load("return (vectors.Vec2.new(2, 3) * 4).x")
        .eval()
        .unwrap();
    assert!((x - 8.0).abs() < 1e-12);
}

#[test]
fn mul_scalar_left() {
    let lua = setup_lua();
    let x: f64 = lua
        .load("return (0.5 * vectors.Vec2.new(6, 10)).x")
        .eval()
        .unwrap();
    assert!((x - 3.0).abs() < 1e-12);
}

#[test]
fn add_scalar_right() {
    let lua = setup_lua();
    let x: f64 = lua
        .load("return (vectors.Vec2.new(1, 2) + 10).x")
        .eval()
        .unwrap();
    assert!((x - 11.0).abs() < 1e-12);
}

#[test]
fn add_scalar_left() {
    let lua = setup_lua();
    let y: f64 = lua
        .load("return (5 + vectors.Vec2.new(1, 2)).y")
        .eval()
        .unwrap();
    assert!((y - 7.0).abs() < 1e-12);
}

#[test]
fn div_scalar() {
    let lua = setup_lua();
    let x: f64 = lua
        .load("return (vectors.Vec2.new(10, 6) / 2).x")
        .eval()
        .unwrap();
    assert!((x - 5.0).abs() < 1e-12);
}

#[test]
fn chained_arithmetic() {
    let lua = setup_lua();
    // (Vec2(1,2) + Vec2(3,4)) * 2 + 1
    let result: f64 = lua
        .load(
            r#"
            local a = vectors.Vec2.new(1, 2)
            local b = vectors.Vec2.new(3, 4)
            local c = (a + b) * 2 + 1
            return c.x
            "#,
        )
        .eval()
        .unwrap();
    // (1+3)*2 + 1 = 9
    assert!((result - 9.0).abs() < 1e-12);
}

// ---------------------------------------------------------------------------
// Free function tests
// ---------------------------------------------------------------------------

#[test]
fn free_fn_add() {
    let lua = setup_lua();
    let result: i64 = lua.load("return vectors.add(3, 4)").eval().unwrap();
    assert_eq!(result, 7);
}

#[test]
fn free_fn_mul() {
    let lua = setup_lua();
    let result: f64 = lua.load("return vectors.mul(2.5, 4.0)").eval().unwrap();
    assert!((result - 10.0).abs() < 1e-12);
}

#[test]
fn free_fn_greet() {
    let lua = setup_lua();
    let result: String = lua.load(r#"return vectors.greet("world")"#).eval().unwrap();
    assert_eq!(result, "hello, world!");
}

#[test]
fn free_fn_in_lua_expression() {
    let lua = setup_lua();
    let result: i64 = lua
        .load("return vectors.add(vectors.add(1, 2), vectors.add(3, 4))")
        .eval()
        .unwrap();
    assert_eq!(result, 10);
}

// ---------------------------------------------------------------------------
// Generic struct tests
// ---------------------------------------------------------------------------

#[derive(Script, Clone)]
#[script(thread_safety = send_sync, methods)]
struct Wrapper<T: Clone + Send + Sync + 'static> {
    value: T,
}

#[script]
impl<T: Clone + Send + Sync + 'static> Wrapper<T> {
    #[script(constructor)]
    fn new(value: T) -> Self {
        Wrapper { value }
    }

    fn get(&self) -> T {
        self.value.clone()
    }
}

fn setup_generic_lua() -> Lua {
    let lua = Lua::new();
    let globals = lua.globals();
    let tbl = lua.create_table().unwrap();
    bind_type::<Wrapper<i32>>(&lua, &tbl).unwrap();
    globals.set("WrapperI32", tbl).unwrap();
    lua
}

#[test]
fn generic_struct_constructor() {
    let lua = setup_generic_lua();
    let result: i64 = lua
        .load("local w = WrapperI32.new(42); return w:get()")
        .eval()
        .unwrap();
    assert_eq!(result, 42);
}

#[test]
fn generic_struct_field_read() {
    let lua = setup_generic_lua();
    let result: i64 = lua
        .load("local w = WrapperI32.new(7); return w.value")
        .eval()
        .unwrap();
    assert_eq!(result, 7);
}

// ---------------------------------------------------------------------------
// Generic free function tests
// ---------------------------------------------------------------------------

#[script(instantiate(i32))]
fn double<T: Clone + std::ops::Add<Output = T>>(x: T) -> T {
    x.clone() + x
}

/// Lua dispatches by name only, so monomorphized instantiations of a generic
/// function are rejected at bind time (generics as a Lua extension are a
/// possible future feature).
#[test]
fn generic_free_fn_is_rejected() {
    let lua = Lua::new();
    let tbl = lua.create_table().unwrap();
    let err = bind_fn::<double>(&lua, &tbl).unwrap_err();
    assert!(err.to_string().contains("generic function `double`"));
}
