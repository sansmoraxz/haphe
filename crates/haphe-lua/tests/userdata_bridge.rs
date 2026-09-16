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
        Display, ToString, PartialEq,
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

// ---------------------------------------------------------------------------
// Operator overload resolution: exact rhs-type match first, then declaration
// order.
// ---------------------------------------------------------------------------

/// f64 overload deliberately declared FIRST: under order-only resolution an
/// integer rhs would bind to it; exact-match-first must pick the i64 one.
#[derive(Script, Clone)]
#[script(traits(Add(rhs = f64, output = Self), Add(rhs = i64, output = Self)))]
struct Probe {
    which: i64,
    value: f64,
}

impl std::ops::Add<f64> for Probe {
    type Output = Probe;
    fn add(self, rhs: f64) -> Probe {
        Probe {
            which: 1,
            value: self.value + rhs,
        }
    }
}

impl std::ops::Add<i64> for Probe {
    type Output = Probe;
    fn add(self, rhs: i64) -> Probe {
        Probe {
            which: 2,
            value: self.value + rhs as f64,
        }
    }
}

/// Only a float overload: an integer rhs has no exact match and falls back
/// to declaration order (lossy integer→float conversion).
#[derive(Script, Clone)]
#[script(traits(Add(rhs = f64, output = Self)))]
struct OnlyFloat {
    value: f64,
}

impl std::ops::Add<f64> for OnlyFloat {
    type Output = OnlyFloat;
    fn add(self, rhs: f64) -> OnlyFloat {
        OnlyFloat {
            value: self.value + rhs,
        }
    }
}

fn setup_overload_lua() -> Lua {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    bind_type::<Probe>(&lua, &table).unwrap();
    let table = lua.create_table().unwrap();
    bind_type::<OnlyFloat>(&lua, &table).unwrap();
    let globals = lua.globals();
    globals
        .set(
            "p",
            lua.create_any_userdata(Probe {
                which: 0,
                value: 10.0,
            })
            .unwrap(),
        )
        .unwrap();
    globals
        .set(
            "of",
            lua.create_any_userdata(OnlyFloat { value: 1.5 }).unwrap(),
        )
        .unwrap();
    lua
}

#[test]
fn integer_rhs_prefers_exact_integer_overload() {
    let lua = setup_overload_lua();
    let which: i64 = lua.load("local q = p + 3; return q.which").eval().unwrap();
    assert_eq!(which, 2, "integer rhs must pick Add(rhs = i64)");
    let value: f64 = lua.load("local q = p + 3; return q.value").eval().unwrap();
    assert_eq!(value, 13.0);
}

#[test]
fn float_rhs_prefers_exact_float_overload() {
    let lua = setup_overload_lua();
    let which: i64 = lua
        .load("local q = p + 2.5; return q.which")
        .eval()
        .unwrap();
    assert_eq!(which, 1, "float rhs must pick Add(rhs = f64)");
    let value: f64 = lua
        .load("local q = p + 2.5; return q.value")
        .eval()
        .unwrap();
    assert_eq!(value, 12.5);
}

#[test]
fn integer_falls_back_to_float_overload_when_no_exact_match() {
    let lua = setup_overload_lua();
    let value: f64 = lua.load("local q = of + 4; return q.value").eval().unwrap();
    assert_eq!(value, 5.5);
}

// ---------------------------------------------------------------------------
// Eq/Ord markers imply the Partial metamethods (core dedupe fix).
// ---------------------------------------------------------------------------

/// Declares ONLY `Eq, Ord` — `__eq`, `__lt`, and `__le` must still bind.
#[derive(Script, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[script(traits(Eq, Ord))]
struct Rank {
    level: i64,
}

/// Declares both markers — `__eq` binds once and works.
#[derive(Script, Clone, PartialEq, Eq)]
#[script(traits(PartialEq, Eq))]
struct Badge {
    id: i64,
}

fn setup_marker_lua() -> Lua {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    bind_type::<Rank>(&lua, &table).unwrap();
    let table = lua.create_table().unwrap();
    bind_type::<Badge>(&lua, &table).unwrap();
    let globals = lua.globals();
    globals
        .set("r1", lua.create_any_userdata(Rank { level: 1 }).unwrap())
        .unwrap();
    globals
        .set("r1b", lua.create_any_userdata(Rank { level: 1 }).unwrap())
        .unwrap();
    globals
        .set("r2", lua.create_any_userdata(Rank { level: 2 }).unwrap())
        .unwrap();
    globals
        .set("b1a", lua.create_any_userdata(Badge { id: 1 }).unwrap())
        .unwrap();
    globals
        .set("b1b", lua.create_any_userdata(Badge { id: 1 }).unwrap())
        .unwrap();
    globals
        .set("b2", lua.create_any_userdata(Badge { id: 2 }).unwrap())
        .unwrap();
    lua
}

#[test]
fn eq_ord_markers_bind_comparison_metamethods() {
    let lua = setup_marker_lua();
    let (eq, ne, lt, nlt, le): (bool, bool, bool, bool, bool) = lua
        .load("return r1 == r1b, r1 == r2, r1 < r2, r2 < r1, r1 <= r1b")
        .eval()
        .unwrap();
    assert!(eq, "__eq via `traits(Eq)` alone");
    assert!(!ne);
    assert!(lt, "__lt via `traits(Ord)` alone");
    assert!(!nlt);
    assert!(le, "__le via `traits(Ord)` alone");
}

#[test]
fn both_eq_markers_bind_once_and_work() {
    let lua = setup_marker_lua();
    let (same, different): (bool, bool) = lua.load("return b1a == b1b, b1a == b2").eval().unwrap();
    assert!(same);
    assert!(!different);
}

// ---------------------------------------------------------------------------
// Display → `..` concatenation with strict string-like operands.
// ---------------------------------------------------------------------------

#[test]
fn concat_with_strings_numbers_and_self() {
    let lua = setup_lua();
    let s: String = lua
        .load(r#"return vectors.Vec2.new(1, 2) .. " tail""#)
        .eval()
        .unwrap();
    assert_eq!(s, "(1, 2) tail");
    let s: String = lua
        .load(r#"return "head " .. vectors.Vec2.new(1, 2)"#)
        .eval()
        .unwrap();
    assert_eq!(s, "head (1, 2)");
    let s: String = lua
        .load("local v = vectors.Vec2.new(1, 2); return v .. v")
        .eval()
        .unwrap();
    assert_eq!(s, "(1, 2)(1, 2)");
    let s: String = lua
        .load("return vectors.Vec2.new(1, 2) .. 42")
        .eval()
        .unwrap();
    assert_eq!(s, "(1, 2)42");
}

#[test]
fn concat_rejects_non_string_like_operands() {
    let lua = setup_lua();
    let err = lua
        .load("return vectors.Vec2.new(1, 2) .. true")
        .eval::<String>()
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("attempt to concatenate a boolean value"),
        "got: {err}"
    );
}

#[test]
fn no_display_means_no_concat() {
    // `Rank` declares no Display: Lua raises its own concat error.
    let lua = setup_marker_lua();
    let err = lua
        .load(r#"return r1 .. "x""#)
        .eval::<String>()
        .unwrap_err();
    assert!(err.to_string().contains("concatenate"), "got: {err}");
}

// ---------------------------------------------------------------------------
// Transparent primitive newtypes cross as native Lua values
// ---------------------------------------------------------------------------

/// TRANSPARENT bool newtype: its values cross as native booleans — Lua
/// truthiness cannot be hooked on userdata (only nil/false are falsy), so
/// bool-like types should be transparent newtypes. An OPAQUE newtype stays a
/// distinct declaration-level type with no generated value conversions;
/// transparency is the opt-in for native representation.
#[derive(Script, Clone, Copy)]
#[script(transparent)]
struct Toggle(bool);

#[derive(Script, Clone, Copy)]
#[script(transparent)]
struct Meters(f64);

#[script]
fn is_ready() -> Toggle {
    Toggle(false)
}

#[script]
fn flip(t: Toggle) -> Toggle {
    Toggle(!t.0)
}

#[script]
fn cruise_altitude() -> Meters {
    Meters(1234.5)
}

#[derive(Script, Clone)]
#[script(methods)]
struct Sensor {
    armed: Toggle,
}

#[script]
impl Sensor {
    #[script(constructor)]
    fn new() -> Self {
        Sensor {
            armed: Toggle(false),
        }
    }

    fn toggled(&self, next: Toggle) -> Toggle {
        Toggle(!next.0)
    }

    fn reading(&self) -> Meters {
        Meters(42.0)
    }
}

#[test]
fn transparent_newtypes_cross_as_native_values_in_free_fns() {
    let lua = Lua::new();
    let tbl = lua.create_table().unwrap();
    bind_fn::<is_ready>(&lua, &tbl).unwrap();
    bind_fn::<flip>(&lua, &tbl).unwrap();
    bind_fn::<cruise_altitude>(&lua, &tbl).unwrap();
    lua.globals().set("m", &tbl).unwrap();

    // Native boolean, participates in truthiness.
    let ty: String = lua.load("return type(m.is_ready())").eval().unwrap();
    assert_eq!(ty, "boolean");
    let negated: bool = lua.load("return not m.is_ready()").eval().unwrap();
    assert!(negated);
    let branch: String = lua
        .load("if m.is_ready() then return 'on' else return 'off' end")
        .eval()
        .unwrap();
    assert_eq!(branch, "off");

    // A fn taking the newtype accepts a plain Lua boolean.
    let flipped: bool = lua.load("return m.flip(false)").eval().unwrap();
    assert!(flipped);

    // Numeric newtype: native number.
    let ty: String = lua.load("return type(m.cruise_altitude())").eval().unwrap();
    assert_eq!(ty, "number");
    let v: f64 = lua.load("return m.cruise_altitude()").eval().unwrap();
    assert_eq!(v, 1234.5);
}

#[test]
fn transparent_newtype_field_and_method_bind_in_lua() {
    let lua = Lua::new();
    let tbl = lua.create_table().unwrap();
    bind_type::<Sensor>(&lua, &tbl).unwrap();
    lua.globals().set("Sensor", &tbl).unwrap();
    lua.load("s = Sensor.new()").exec().unwrap();

    // Field crosses as a native boolean (and truthiness works).
    let ty: String = lua.load("return type(s.armed)").eval().unwrap();
    assert_eq!(ty, "boolean");
    let negated: bool = lua.load("return not s.armed").eval().unwrap();
    assert!(negated);

    // Field write accepts a plain boolean.
    lua.load("s.armed = true").exec().unwrap();
    let armed: bool = lua.load("return s.armed").eval().unwrap();
    assert!(armed);

    // Method param + return through the newtype.
    let toggled: bool = lua.load("return s:toggled(false)").eval().unwrap();
    assert!(toggled);
    let ty: String = lua.load("return type(s:reading())").eval().unwrap();
    assert_eq!(ty, "number");
    let v: f64 = lua.load("return s:reading()").eval().unwrap();
    assert_eq!(v, 42.0);
}
