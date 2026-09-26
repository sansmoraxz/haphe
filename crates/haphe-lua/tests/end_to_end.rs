//! Full integration test: derive macros → registry! → validate → `LuaBinder` → Lua.
//!
//! Exercises the exact workflow an external user would follow. Every type is
//! defined with `#[derive(Script)]` and `#[script]`, assembled by
//! `registry!`, and bound into a live Lua runtime.

#![allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    clippy::doc_markdown,
    clippy::trivially_copy_pass_by_ref,
    reason = "fixture shapes are dictated by the bridge surface under test: `#[script]` functions receive OWNED values (the boundary contract), methods keep their declared receivers (`&self` on Copy enums included), and docs name fixture idents verbatim"
)]
#![allow(
    dead_code,
    clippy::approx_constant,
    reason = "fixtures are exercised through generated bindings, and the PI fixture constant deliberately spells out the approximate value under test"
)]

use haphe::{RuntimeBinder, Script, script};
use haphe_lua::LuaBinder;
use mlua::Lua;

// ---------------------------------------------------------------------------
// Domain types — what a user would write
// ---------------------------------------------------------------------------

/// A 2D point.
#[derive(Script, Clone)]
#[script(thread_safety = send_sync, traits(PartialEq, Display, Clone), methods)]
struct Point {
    x: f64,
    #[script(readonly)]
    y: f64,
}

impl std::fmt::Display for Point {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {})", self.x, self.y)
    }
}

impl PartialEq for Point {
    fn eq(&self, other: &Self) -> bool {
        self.x == other.x && self.y == other.y
    }
}

#[script]
impl Point {
    #[script(constructor)]
    fn new(x: f64, y: f64) -> Self {
        Point { x, y }
    }

    fn distance_to(&self, other: &Point) -> f64 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }

    /// The point's distance from the origin.
    #[script(getter)]
    fn length(&self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
}

/// A named color.
#[derive(Script)]
#[script(traits(Debug))]
enum Color {
    Red,
    Green,
    Blue,
    Rgb(u8, u8, u8),
}

impl std::fmt::Debug for Color {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Color::Red => write!(f, "Red"),
            Color::Green => write!(f, "Green"),
            Color::Blue => write!(f, "Blue"),
            Color::Rgb(r, g, b) => write!(f, "Rgb({r}, {g}, {b})"),
        }
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

/// Greets someone by name.
#[script]
fn greet(name: String) -> String {
    format!("hello, {name}!")
}

// ---------------------------------------------------------------------------
// Registry — assembles everything into modules
// ---------------------------------------------------------------------------

haphe::registry! {
    pub static REGISTRY = {
        structs: [Point],
        enums: [Color],
        modules: [
            mod geometry {
                doc: "Geometry types and utilities",
                functions: [add, mul],
                types: [Point, Color],
                constants: [
                    /// The ratio of a circle's circumference to its diameter.
                    PI: f64 = 3.141_592_653_589_793,
                    /// Maximum number of vertices.
                    MAX_VERTICES: i32 = 1024,
                    /// Whether debug drawing is on.
                    DEBUG: bool = false,
                    /// Library version.
                    VERSION: &str = "0.1.0",
                ],
                modules: [
                    mod utils {
                        doc: "Utility helpers",
                        functions: [greet],
                    },
                ],
            },
        ],
    };
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Creates a bound Lua state from the static registry.
fn bound_lua() -> Lua {
    let mut lua = Lua::new();
    let binder = LuaBinder::new();
    let validated = REGISTRY.validate().expect("registry validates");
    binder.bind(&validated, &mut lua).expect("binding succeeds");
    lua
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn registry_validates() {
    REGISTRY.validate().expect("registry is structurally valid");
}

/// Without `send`, no thread-safety constraint — the full registry passes.
#[test]
#[cfg(not(feature = "send"))]
fn binder_capabilities_accept_registry() {
    let validated = REGISTRY.validate().unwrap();
    let binder = LuaBinder::new();
    binder
        .capabilities()
        .check(&validated)
        .expect("binder capabilities accept this registry");
}

/// With `send`, the binder requires `Send` on all types — `Color`
/// (`thread_safety = none`) is correctly rejected.
#[test]
#[cfg(feature = "send")]
fn binder_capabilities_reject_non_send_types() {
    let validated = REGISTRY.validate().unwrap();
    let binder = LuaBinder::new();
    let errors = binder
        .capabilities()
        .check(&validated)
        .expect_err("Color should fail the Send requirement");
    assert_eq!(errors.len(), 1, "only Color should fail: {errors:?}");
}

#[test]
fn top_level_module_exists() {
    let lua = bound_lua();
    let _geo: mlua::Table = lua.globals().get("geometry").expect("geometry global");
}

#[test]
fn constants_are_bound_with_correct_types() {
    let lua = bound_lua();
    let geo: mlua::Table = lua.globals().get("geometry").unwrap();

    let pi: f64 = geo.get("PI").unwrap();
    assert!((pi - std::f64::consts::PI).abs() < 1e-12);

    let max: i64 = geo.get("MAX_VERTICES").unwrap();
    assert_eq!(max, 1024);

    let debug: bool = geo.get("DEBUG").unwrap();
    assert!(!debug);

    let version: String = geo.get("VERSION").unwrap();
    assert_eq!(version, "0.1.0");
}

#[test]
fn lua_script_reads_constants() {
    let lua = bound_lua();

    let result: bool = lua
        .load("return geometry.PI > 3 and geometry.MAX_VERTICES == 1024")
        .eval()
        .unwrap();
    assert!(result);
}

#[test]
fn type_stubs_are_present() {
    let lua = bound_lua();
    let geo: mlua::Table = lua.globals().get("geometry").unwrap();

    // Both types are registered as table entries.
    let _point: mlua::Table = geo.get("Point").expect("Point stub");
    let _color: mlua::Table = geo.get("Color").expect("Color stub");
}

#[test]
fn submodule_is_nested() {
    let lua = bound_lua();
    let geo: mlua::Table = lua.globals().get("geometry").unwrap();
    let _utils: mlua::Table = geo.get("utils").expect("nested utils module");
}

#[test]
fn submodule_functions_are_stubbed() {
    let lua = bound_lua();
    let geo: mlua::Table = lua.globals().get("geometry").unwrap();
    let utils: mlua::Table = geo.get("utils").unwrap();

    // The stub exists as a function value.
    let greet_val: mlua::Value = utils.get("greet").unwrap();
    assert!(
        matches!(greet_val, mlua::Value::Function(_)),
        "expected a function stub"
    );
}

#[test]
fn function_stubs_error_on_call() {
    let lua = bound_lua();

    let err = lua
        .load("geometry.add(1, 2)")
        .exec()
        .expect_err("stub should error");
    let msg = err.to_string();
    assert!(msg.contains("not bound"), "got: {msg}");
}

#[test]
fn submodule_function_stubs_error_on_call() {
    let lua = bound_lua();

    let err = lua
        .load("geometry.utils.greet('world')")
        .exec()
        .expect_err("stub should error");
    let msg = err.to_string();
    assert!(msg.contains("not bound"), "got: {msg}");
}

#[test]
fn multiple_modules_independent() {
    // Binding twice into the same state replaces the global, no crash.
    let mut lua = Lua::new();
    let binder = LuaBinder::new();
    let validated = REGISTRY.validate().unwrap();
    binder.bind(&validated, &mut lua).unwrap();
    binder.bind(&validated, &mut lua).unwrap();

    let pi: f64 = lua.load("return geometry.PI").eval().unwrap();
    assert!((pi - std::f64::consts::PI).abs() < 1e-12);
}

#[test]
fn custom_capabilities() {
    use haphe::BackendCapabilities;

    // A backend that disables async still accepts this registry (no async fns).
    let binder = LuaBinder::with_capabilities(BackendCapabilities::ALL.with_async_fns(false));
    let validated = REGISTRY.validate().unwrap();
    binder
        .capabilities()
        .check(&validated)
        .expect("no async fns in this registry");

    let mut lua = Lua::new();
    binder.bind(&validated, &mut lua).unwrap();
}

#[test]
fn language_name() {
    let binder = LuaBinder::new();
    assert_eq!(binder.language_name(), "lua");
}

#[test]
fn lua_table_iteration() {
    let lua = bound_lua();

    // Collect all keys from the geometry module table.
    let keys: Vec<String> = lua
        .load(
            r"
            local keys = {}
            for k, _ in pairs(geometry) do
                keys[#keys + 1] = k
            end
            table.sort(keys)
            return keys
            ",
        )
        .eval()
        .unwrap();

    // Constants, type stubs, functions, and submodules all appear.
    assert!(keys.contains(&"PI".to_string()));
    assert!(keys.contains(&"MAX_VERTICES".to_string()));
    assert!(keys.contains(&"DEBUG".to_string()));
    assert!(keys.contains(&"VERSION".to_string()));
    assert!(keys.contains(&"Point".to_string()));
    assert!(keys.contains(&"Color".to_string()));
    assert!(keys.contains(&"add".to_string()));
    assert!(keys.contains(&"mul".to_string()));
    assert!(keys.contains(&"utils".to_string()));
}

#[test]
fn table_constructor_shorthand() {
    let lua = Lua::new();
    let tbl = lua.create_table().unwrap();
    haphe_lua::bind_type::<Point>(&lua, &tbl).unwrap();
    lua.globals().set("Point", tbl).unwrap();

    let (x, y): (f64, f64) = lua
        .load(
            r"
            local p = Point(3.0, 4.0)
            return p.x, p.y
            ",
        )
        .eval()
        .unwrap();

    assert!((x - 3.0).abs() < f64::EPSILON);
    assert!((y - 4.0).abs() < f64::EPSILON);
}
