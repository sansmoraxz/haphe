//! Enum exposure: populated case tables, repr-driven value boundary, and
//! methods-bearing enums as userdata.

#![allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    clippy::doc_markdown,
    clippy::trivially_copy_pass_by_ref,
    reason = "fixture shapes are dictated by the bridge surface under test: `#[script]` functions receive OWNED values (the boundary contract), methods keep their declared receivers (`&self` on Copy enums included), and docs name fixture idents verbatim"
)]

use haphe::{RuntimeBinder, Script, registry, script};
use haphe_rhai::{RhaiBinder, bind_enum_type};

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

/// A methods-bearing enum, bound as userdata.
#[derive(Script, Clone, Copy, PartialEq, Debug)]
#[script(thread_safety = send_sync, methods)]
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

registry! {
    pub static REGISTRY = {
        enums: [Color, Level, Shape],
        modules: [
            mod palette {
                types: [Color, Level, Shape],
            },
        ],
    };
}

fn bound_engine() -> rhai::Engine {
    let mut engine = rhai::Engine::new();
    let binder = RhaiBinder::new();
    let validated = REGISTRY.validate().expect("registry validates");
    binder.bind(&validated, &mut engine).expect("binding succeeds");
    engine
}

#[test]
fn case_constants_are_populated() {
    let engine = bound_engine();
    let red: String = engine.eval("palette::Color::Red").unwrap();
    assert_eq!(red, "Red");
    let grey: String = engine.eval("palette::Color::grey").unwrap();
    assert_eq!(grey, "grey");
    let low: i64 = engine.eval("palette::Level::Low").unwrap();
    assert_eq!(low, 1);
    let high: i64 = engine.eval("palette::Level::High").unwrap();
    assert_eq!(high, 10);
}

#[test]
fn numeric_enum_roundtrips_as_integer() {
    let engine = bound_engine();
    let mid: i64 = engine.eval("palette::Level::Mid").unwrap();
    assert_eq!(mid, 2);
    let low: i64 = engine.eval("palette::Level::Low").unwrap();
    assert_eq!(low, 1);
    let high: i64 = engine.eval("palette::Level::High").unwrap();
    assert_eq!(high, 10);
}

#[test]
fn string_enum_travels_by_name() {
    let engine = bound_engine();
    let red: String = engine.eval("palette::Color::Red").unwrap();
    assert_eq!(red, "Red");
    let grey: String = engine.eval("palette::Color::grey").unwrap();
    assert_eq!(grey, "grey");
}

#[test]
fn methods_enum_binds_as_custom_type() {
    let mut engine = bound_engine();
    let mut type_mod = rhai::Module::new();
    bind_enum_type::<Shape>(&mut engine, &mut type_mod).expect("enum userdata binds");

    let mut scope = rhai::Scope::new();
    scope.push("w", Shape::Wide);
    let area: f64 = engine
        .eval_with_scope(&mut scope, "w.area()")
        .expect("eval");
    assert_eq!(area, 2.0);
}
