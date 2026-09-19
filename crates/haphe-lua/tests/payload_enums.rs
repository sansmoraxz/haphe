//! Payload enums: cases with values cross as case tables
//! (`{ case = "Name", <values at [1..n]> }`), constructed script-side through
//! per-case functions on the enum's type table; unit cases keep their
//! constant representation exactly.

#![allow(dead_code)]

use haphe::{RuntimeBinder, Script, script};
use haphe_lua::{LuaBinder, LuaDeclGenerator, bind_fn, bind_type};
use mlua::Lua;

/// A drawable shape.
#[derive(Script, Clone, PartialEq, Debug)]
enum Shape {
    Empty,
    Circle(f64),
    Rect { width: f64, height: f64 },
    Label(String, i64),
}

/// A drawing surface.
#[derive(Script, Clone)]
#[script(methods)]
struct Canvas {
    #[script(readonly)]
    scale: f64,
}

#[script]
impl Canvas {
    #[script(constructor)]
    fn new(scale: f64) -> Self {
        Canvas { scale }
    }

    /// Payload enum as a method parameter.
    fn area(&self, shape: Shape) -> f64 {
        match shape {
            Shape::Empty => 0.0,
            Shape::Circle(r) => r * r,
            Shape::Rect { width, height } => width * height,
            Shape::Label(_, n) => n as f64,
        }
    }

    /// Payload enum as a method return (struct case).
    fn scaled_rect(&self, width: f64, height: f64) -> Shape {
        Shape::Rect {
            width: width * self.scale,
            height: height * self.scale,
        }
    }

    /// Unit case returns keep the plain representation.
    fn nothing(&self) -> Shape {
        Shape::Empty
    }
}

/// Builds a label shape (tuple case through a free function).
#[script]
fn label(text: String, weight: i64) -> Shape {
    Shape::Label(text, weight)
}

haphe::registry! {
    pub static REGISTRY = {
        structs: [Canvas],
        enums: [Shape],
        modules: [
            mod draw {
                functions: [label],
                types: [Canvas, Shape],
            },
        ],
    };
}

fn setup_lua() -> Lua {
    let mut lua = Lua::new();
    let binder = LuaBinder::new();
    let validated = REGISTRY.validate().unwrap();
    binder.bind(&validated, &mut lua).unwrap();

    let draw: mlua::Table = lua.globals().get("draw").unwrap();
    let canvas_table: mlua::Table = draw.get("Canvas").unwrap();
    bind_type::<Canvas>(&lua, &canvas_table).unwrap();
    bind_fn::<label>(&lua, &draw).unwrap();
    lua
}

#[test]
fn payload_cases_construct_and_convert_as_params() {
    let lua = setup_lua();
    let area: f64 = lua
        .load("local c = draw.Canvas.new(1.0) return c:area(draw.Shape.Circle(3.0))")
        .eval()
        .unwrap();
    assert_eq!(area, 9.0);
    // Struct case: constructor takes the fields in declaration order.
    let area: f64 = lua
        .load("local c = draw.Canvas.new(1.0) return c:area(draw.Shape.Rect(2.0, 4.0))")
        .eval()
        .unwrap();
    assert_eq!(area, 8.0);
    // A hand-written case table works identically.
    let area: f64 = lua
        .load("local c = draw.Canvas.new(1.0) return c:area({ case = \"Circle\", 5.0 })")
        .eval()
        .unwrap();
    assert_eq!(area, 25.0);
}

#[test]
fn unit_cases_keep_the_plain_representation() {
    let lua = setup_lua();
    // The unit case constant is the case-name string, as before.
    let empty: String = lua.load("return draw.Shape.Empty").eval().unwrap();
    assert_eq!(empty, "Empty");
    let area: f64 = lua
        .load("local c = draw.Canvas.new(1.0) return c:area(draw.Shape.Empty)")
        .eval()
        .unwrap();
    assert_eq!(area, 0.0);
    // Unit returns cross as the plain string too.
    let out: String = lua
        .load("local c = draw.Canvas.new(1.0) return c:nothing()")
        .eval()
        .unwrap();
    assert_eq!(out, "Empty");
}

#[test]
fn payload_returns_cross_as_case_tables() {
    let lua = setup_lua();
    let (case, w, h): (String, f64, f64) = lua
        .load(
            "local c = draw.Canvas.new(2.0)\n\
             local r = c:scaled_rect(2.0, 3.0)\n\
             return r.case, r[1], r[2]",
        )
        .eval()
        .unwrap();
    assert_eq!(case, "Rect");
    assert_eq!(w, 4.0);
    assert_eq!(h, 6.0);
}

#[test]
fn tuple_cases_roundtrip_through_free_fns() {
    let lua = setup_lua();
    let (case, text, weight): (String, String, i64) = lua
        .load(
            "local l = draw.label(\"tag\", 7)\n\
             return l.case, l[1], l[2]",
        )
        .eval()
        .unwrap();
    assert_eq!(case, "Label");
    assert_eq!(text, "tag");
    assert_eq!(weight, 7);
    // And back in as a parameter.
    let area: f64 = lua
        .load("local c = draw.Canvas.new(1.0) return c:area(draw.label(\"x\", 3))")
        .eval()
        .unwrap();
    assert_eq!(area, 3.0);
}

#[test]
fn constructor_arity_and_case_errors_are_descriptive() {
    let lua = setup_lua();
    let err = lua
        .load("return draw.Shape.Circle()")
        .eval::<mlua::Value>()
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("Shape.Circle expects 1 value(s), got 0"),
        "got: {err}"
    );
    // Unknown case names error from conversion.
    let err = lua
        .load("local c = draw.Canvas.new(1.0) return c:area({ case = \"Blob\", 1.0 })")
        .eval::<f64>()
        .unwrap_err()
        .to_string();
    assert!(err.contains("unknown enum case"), "got: {err}");
    // A payload case with the wrong arity errors from conversion.
    let err = lua
        .load("local c = draw.Canvas.new(1.0) return c:area({ case = \"Circle\" })")
        .eval::<f64>()
        .unwrap_err()
        .to_string();
    assert!(err.contains("payload of mismatched length"), "got: {err}");
}

#[test]
fn decl_stubs_describe_payload_cases_as_constructors() {
    use haphe::BindingGenerator;
    let validated = REGISTRY.validate().unwrap();
    let output = LuaDeclGenerator::new().generate(&validated).unwrap();
    let s = String::from_utf8(output.files[0].content.clone()).unwrap();
    assert!(s.contains("---@class Shape"), "got:\n{s}");
    assert!(s.contains("    Empty = \"Empty\","), "got:\n{s}");
    assert!(s.contains("---@param arg1 number"), "got:\n{s}");
    assert!(s.contains("function Shape.Circle(arg1) end"), "got:\n{s}");
    assert!(s.contains("---@param width number"), "got:\n{s}");
    assert!(s.contains("---@param height number"), "got:\n{s}");
    assert!(
        s.contains("function Shape.Rect(width, height) end"),
        "got:\n{s}"
    );
    assert!(
        s.contains("function Shape.Label(arg1, arg2) end"),
        "got:\n{s}"
    );
    assert!(s.contains("---@return Shape"), "got:\n{s}");
}
