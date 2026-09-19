//! Enum exposure: populated case tables, repr-driven value boundary, and
//! methods-bearing enums as userdata.

#![allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    clippy::doc_markdown,
    clippy::trivially_copy_pass_by_ref,
    reason = "fixture shapes are dictated by the bridge surface under test: `#[script]` functions receive OWNED values (the boundary contract), methods keep their declared receivers (`&self` on Copy enums included), and docs name fixture idents verbatim"
)]

use haphe::{RuntimeBinder, Script, script};
use haphe_lua::{LuaBinder, bind_fn};
use mlua::Lua;

/// A string-represented enum (no `#[repr]`).
#[derive(Script, Clone, Copy, PartialEq, Debug)]
enum Color {
    Red,
    #[script(rename = "grey")]
    Gray,
}

/// A numeric enum: the Rust `#[repr(u8)]` propagates.
#[derive(Script, Clone, Copy, PartialEq, Debug)]
#[repr(u8)]
enum Level {
    Low = 1,
    Mid,
    High = 10,
}

/// Echoes a level.
#[script]
fn through(level: Level) -> Level {
    level
}

/// Names a color.
#[script]
fn describe(color: Color) -> String {
    format!("{color:?}")
}

/// A methods-bearing enum, bound as userdata.
#[derive(Script, Clone, Copy, PartialEq, Debug)]
#[script(methods)]
enum Shape {
    Unit,
    Wide,
}

#[script]
impl Shape {
    fn area(&self) -> f64 {
        match self {
            Shape::Unit => 1.0,
            Shape::Wide => 2.0,
        }
    }
}

/// Produces a shape value for scripts.
#[script]
fn wide() -> Shape {
    Shape::Wide
}

haphe::registry! {
    pub static REGISTRY = {
        enums: [Color, Level, Shape],
        modules: [
            mod palette {
                functions: [through, describe, wide],
                types: [Color, Level, Shape],
            },
        ],
    };
}

fn bound_lua() -> Lua {
    let mut lua = Lua::new();
    let binder = LuaBinder::new();
    let validated = REGISTRY.validate().expect("registry validates");
    binder.bind(&validated, &mut lua).expect("binding succeeds");
    // Replace the module's function stubs with live bindings.
    let palette: mlua::Table = lua.globals().get("palette").unwrap();
    bind_fn::<through>(&lua, &palette).unwrap();
    bind_fn::<describe>(&lua, &palette).unwrap();
    bind_fn::<wide>(&lua, &palette).unwrap();
    lua
}

#[test]
fn case_tables_are_populated() {
    let lua = bound_lua();
    let red: String = lua.load("return palette.Color.Red").eval().unwrap();
    assert_eq!(red, "Red");
    let grey: String = lua.load("return palette.Color.grey").eval().unwrap();
    assert_eq!(grey, "grey");
    // Numeric enums: integer discriminants, exact values.
    let low: i64 = lua.load("return palette.Level.Low").eval().unwrap();
    assert_eq!(low, 1);
    let high: i64 = lua.load("return palette.Level.High").eval().unwrap();
    assert_eq!(high, 10);
}

#[test]
fn numeric_enum_roundtrips_as_integer() {
    let lua = bound_lua();
    let out: i64 = lua
        .load("return palette.through(palette.Level.Mid)")
        .eval()
        .unwrap();
    assert_eq!(out, 2);
    // Plain integers convert too.
    let out: i64 = lua.load("return palette.through(10)").eval().unwrap();
    assert_eq!(out, 10);
    // Unknown discriminant errors.
    assert!(lua.load("return palette.through(3)").eval::<i64>().is_err());
}

#[test]
fn string_enum_travels_by_name_and_rejects_integers() {
    let lua = bound_lua();
    let out: String = lua
        .load("return palette.describe(palette.Color.Red)")
        .eval()
        .unwrap();
    assert_eq!(out, "Red");
    assert!(
        lua.load("return palette.describe(0)")
            .eval::<String>()
            .is_err()
    );
}

#[test]
fn methods_enum_binds_as_userdata() {
    let lua = bound_lua();
    let table: mlua::Table = lua
        .load("return palette.Shape")
        .eval()
        .expect("type table exists");
    haphe_lua::bind_enum_type::<Shape>(&lua, &table).expect("enum userdata binds");

    // A function returning the enum yields the lightweight value (`wide()`
    // returns the case name string); the userdata surface is the receiver
    // for methods, exercised by pushing a userdata value directly.
    let value: String = lua.load("return palette.wide()").eval().unwrap();
    assert_eq!(value, "Wide");
    let ud = lua.create_any_userdata(Shape::Wide).unwrap();
    lua.globals().set("wide_ud", ud).unwrap();
    let area: f64 = lua.load("return wide_ud:area()").eval().unwrap();
    assert_eq!(area, 2.0);
}
