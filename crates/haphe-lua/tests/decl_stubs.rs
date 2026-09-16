//! LuaLS declaration-stub generation: text assertions over the `---@meta`
//! file, mirroring what the runtime binder exposes.

#![allow(dead_code)]

use haphe::{Script, script};
use haphe_lua::{LuaDeclError, LuaDeclGenerator};

/// A 2D point.
#[derive(Script, Clone)]
#[script(
    traits(Display, ToString, Add, Add(rhs = f64, output = Self), Neg(output = Self)),
    methods
)]
struct Point {
    x: f64,
    /// Vertical position.
    #[script(readonly)]
    y: f64,
}

impl std::fmt::Display for Point {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {})", self.x, self.y)
    }
}

impl std::ops::Add for Point {
    type Output = Point;
    fn add(self, rhs: Point) -> Point {
        Point {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
        }
    }
}

impl std::ops::Add<f64> for Point {
    type Output = Point;
    fn add(self, rhs: f64) -> Point {
        Point {
            x: self.x + rhs,
            y: self.y + rhs,
        }
    }
}

impl std::ops::Neg for Point {
    type Output = Point;
    fn neg(self) -> Point {
        Point {
            x: -self.x,
            y: -self.y,
        }
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
}

/// A named color.
#[derive(Script)]
enum Color {
    Red,
    #[script(rename = "grey")]
    Gray,
}

/// Looks up a color.
#[script]
fn pick(name: String, fallback: Option<i64>) -> Vec<i64> {
    let _ = (name, fallback);
    Vec::new()
}

/// Tags entries.
#[script(error_kind = "TagError")]
fn tag(entries: std::collections::HashMap<String, i64>) -> Result<String, String> {
    let _ = entries;
    Ok(String::new())
}

/// Visits each sample.
#[script]
fn each(cb: fn(i64) -> bool) {
    let _ = cb;
}

/// Host-side hooks.
#[script(foreign, thread_safety = none)]
pub trait Hooks {
    /// Reports progress.
    fn progress(&self, done: i64, total: i64);

    #[script(error_kind = "IoError")]
    fn fetch(&self, url: String) -> Result<String, HookError>;
}

pub struct HookError;

impl From<haphe::ForeignError> for HookError {
    fn from(_: haphe::ForeignError) -> Self {
        HookError
    }
}

haphe::registry! {
    pub static REGISTRY = {
        structs: [Point],
        enums: [Color],
        foreign: [HooksHandle],
        modules: [
            mod geometry {
                doc: "Geometry utilities",
                functions: [pick, tag, each],
                types: [Point, Color],
                constants: [
                    /// Maximum number of vertices.
                    MAX_VERTICES: i64 = 1024,
                ],
                modules: [
                    mod inner { functions: [pick] },
                ],
            },
        ],
    };
}

fn generate() -> String {
    let output = haphe::generate(&LuaDeclGenerator::new(), &REGISTRY).expect("generation succeeds");
    assert_eq!(output.files.len(), 1);
    assert_eq!(output.files[0].path, "definitions.lua");
    String::from_utf8(output.files[0].content.clone()).unwrap()
}

#[test]
fn stub_is_a_meta_file_and_deterministic() {
    let a = generate();
    let b = generate();
    assert_eq!(a, b, "generation must be byte-deterministic");
    assert!(a.starts_with("---@meta\n"), "got:\n{a}");
}

#[test]
fn classes_fields_and_operators() {
    let s = generate();
    assert!(s.contains("---A 2D point.\n---@class Point"), "got:\n{s}");
    assert!(s.contains("---@field x number\n"), "got:\n{s}");
    assert!(
        s.contains("---@field y number Readonly in the source API."),
        "got:\n{s}"
    );
    assert!(s.contains("---@operator add(Point): Point"), "got:\n{s}");
    assert!(s.contains("---@operator add(number): Point"), "got:\n{s}");
    assert!(s.contains("---@operator unm: Point"), "got:\n{s}");
    assert!(
        s.contains("---@operator concat(string|number): string"),
        "got:\n{s}"
    );
    assert!(s.contains("local Point = {}"), "got:\n{s}");
    // Colon-syntax method with resolved parameter type.
    assert!(s.contains("---@param other Point"), "got:\n{s}");
    assert!(
        s.contains("function Point:distance_to(other) end"),
        "got:\n{s}"
    );
}

#[test]
fn constructors_live_on_the_module_type_table() {
    let s = generate();
    assert!(s.contains("geometry.Point = {}"), "got:\n{s}");
    assert!(
        s.contains("function geometry.Point.new(x, y) end"),
        "got:\n{s}"
    );
}

#[test]
fn unit_enum_alias_uses_declared_case_names() {
    let s = generate();
    assert!(s.contains("---@alias Color \"Red\"|\"grey\""), "got:\n{s}");
}

#[test]
fn module_functions_and_types() {
    let s = generate();
    assert!(
        s.contains("---Geometry utilities\ngeometry = {}"),
        "got:\n{s}"
    );
    assert!(s.contains("---@param name string"), "got:\n{s}");
    assert!(s.contains("---@param fallback integer?"), "got:\n{s}");
    assert!(s.contains("---@return integer[]"), "got:\n{s}");
    assert!(
        s.contains("function geometry.pick(name, fallback) end"),
        "got:\n{s}"
    );
    // Map + Result-return + error_kind doc.
    assert!(
        s.contains("---@param entries table<string, integer>"),
        "got:\n{s}"
    );
    assert!(s.contains("---Errors raise (TagError)."), "got:\n{s}");
    assert!(
        s.contains("function geometry.tag(entries) end"),
        "got:\n{s}"
    );
    // Callback param.
    assert!(
        s.contains("---@param cb fun(a1: integer): boolean"),
        "got:\n{s}"
    );
    // Nested module.
    assert!(s.contains("geometry.inner = {}"), "got:\n{s}");
    assert!(
        s.contains("function geometry.inner.pick(name, fallback) end"),
        "got:\n{s}"
    );
}

#[test]
fn constants_are_documented_fields() {
    let s = generate();
    assert!(s.contains("---Maximum number of vertices."), "got:\n{s}");
    assert!(s.contains("---Constant value: 1024"), "got:\n{s}");
    assert!(
        s.contains("---@type integer\ngeometry.MAX_VERTICES = nil"),
        "got:\n{s}"
    );
}

#[test]
fn foreign_interface_describes_the_callbacks_table() {
    let s = generate();
    assert!(
        s.contains("---Callbacks table the script supplies to the host (foreign interface)."),
        "got:\n{s}"
    );
    assert!(s.contains("---@class Hooks"), "got:\n{s}");
    assert!(
        s.contains("---@field progress fun(done: integer, total: integer)"),
        "got:\n{s}"
    );
    assert!(
        s.contains("---@field fetch fun(url: string): string May raise (IoError)."),
        "got:\n{s}"
    );
}

#[test]
fn generic_module_function_is_rejected() {
    /// Echoes a value.
    #[script(instantiate(i64))]
    fn echo<T>(value: T) -> T {
        value
    }

    haphe::registry! {
        static GENERIC_REGISTRY = {
            modules: [ mod util { functions: [echo] } ],
        };
    }

    let err = haphe::generate(&LuaDeclGenerator::new(), &GENERIC_REGISTRY).unwrap_err();
    let haphe::GenerateError::Backend(LuaDeclError::GenericFunction { name }) = err else {
        panic!("expected GenericFunction, got: {err:?}");
    };
    assert_eq!(name, "echo");
}

#[test]
fn duplicate_exposed_type_names_are_rejected() {
    mod a {
        use haphe::Script;
        /// First.
        #[derive(Script)]
        #[script(rename = "Twin")]
        pub struct One {
            pub n: i64,
        }
    }
    mod b {
        use haphe::Script;
        /// Second.
        #[derive(Script)]
        #[script(rename = "Twin")]
        pub struct Two {
            pub n: i64,
        }
    }

    haphe::registry! {
        static TWIN_REGISTRY = {
            structs: [a::One, b::Two],
        };
    }

    let err = haphe::generate(&LuaDeclGenerator::new(), &TWIN_REGISTRY).unwrap_err();
    assert!(
        matches!(
            err,
            haphe::GenerateError::Backend(LuaDeclError::DuplicateTypeName { ref name })
                if name == "Twin"
        ),
        "got: {err:?}"
    );
}
