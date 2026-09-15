//! Full integration test: derive macros → registry! → haphe::generate →
//! WIT text assertions.

#![allow(dead_code, clippy::approx_constant)]

use haphe::{BindingGenerator, Script, script};
use haphe_wit::{ConstantMode, WitGenerator};

// ---------------------------------------------------------------------------
// Domain types
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

/// A plain data record with no behavior.
#[derive(Script)]
struct Size {
    width: u32,
    height: u32,
}

/// A named color.
#[derive(Script)]
enum Color {
    Red,
    Green,
    Blue,
    Rgb(u8, u8, u8),
}

/// Cardinal directions.
#[derive(Script)]
enum Direction {
    North,
    South,
    East,
    West,
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

/// Midpoint of two points.
#[script]
fn midpoint(a: &Point, b: &Point) -> Point {
    Point {
        x: (a.x + b.x) / 2.0,
        y: (a.y + b.y) / 2.0,
    }
}

/// Fetches remote data.
#[script]
async fn fetch_data(url: String) -> String {
    url
}

/// Parses a point from text.
#[script]
fn parse_point(text: String) -> Result<Point, String> {
    let (x, y) = text.split_once(',').ok_or("bad format")?;
    Ok(Point {
        x: x.trim().parse().map_err(|e| format!("{e}"))?,
        y: y.trim().parse().map_err(|e| format!("{e}"))?,
    })
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

haphe::registry! {
    pub static REGISTRY = {
        structs: [Point, Size],
        enums: [Color, Direction],
        modules: [
            mod geometry {
                doc: "Geometry types and utilities",
                functions: [add, mul, midpoint, parse_point, fetch_data],
                types: [Point, Color],
                constants: [
                    /// The ratio of a circle's circumference to its diameter.
                    PI: f64 = 3.141592653589793,
                    /// Maximum number of vertices.
                    MAX_VERTICES: i32 = 1024,
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

fn generate() -> String {
    let generator = WitGenerator::new("haphe:demo");
    let output = haphe::generate(&generator, &REGISTRY).expect("generation succeeds");
    assert_eq!(output.files.len(), 1);
    assert_eq!(output.files[0].path, "wit/host.wit");
    String::from_utf8(output.files[0].content.clone()).unwrap()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn language_name_and_capabilities() {
    let generator = WitGenerator::new("haphe:demo");
    assert_eq!(generator.language_name(), "wit");
    let validated = REGISTRY.validate().unwrap();
    generator
        .capabilities()
        .check(&validated)
        .expect("registry has no callbacks/generics");
}

#[test]
fn package_header() {
    let wit = generate();
    assert!(wit.starts_with("package haphe:demo;\n"), "got:\n{wit}");
}

#[test]
fn package_version() {
    let generator = WitGenerator::new("haphe:demo").with_version("1.2.3");
    let output = haphe::generate(&generator, &REGISTRY).unwrap();
    let wit = String::from_utf8(output.files[0].content.clone()).unwrap();
    assert!(
        wit.starts_with("package haphe:demo@1.2.3;\n"),
        "got:\n{wit}"
    );
}

#[test]
fn point_is_a_resource() {
    let wit = generate();
    assert!(wit.contains("resource point {"), "got:\n{wit}");
    assert!(wit.contains("constructor(x: f64, y: f64);"), "got:\n{wit}");
    // Writable field gets getter + setter; readonly only a getter.
    assert!(wit.contains("x: func() -> f64;"), "got:\n{wit}");
    assert!(wit.contains("set-x: func(value: f64);"), "got:\n{wit}");
    assert!(wit.contains("y: func() -> f64;"), "got:\n{wit}");
    assert!(!wit.contains("set-y:"), "got:\n{wit}");
    // Borrowed resource param.
    assert!(
        wit.contains("distance-to: func(other: borrow<point>) -> f64;"),
        "got:\n{wit}"
    );
}

#[test]
fn size_is_a_record() {
    let wit = generate();
    assert!(wit.contains("record size {"), "got:\n{wit}");
    assert!(wit.contains("width: u32,"), "got:\n{wit}");
    assert!(wit.contains("height: u32,"), "got:\n{wit}");
}

#[test]
fn unit_enum_and_variant() {
    let wit = generate();
    assert!(wit.contains("enum direction {"), "got:\n{wit}");
    assert!(wit.contains("north,"), "got:\n{wit}");
    assert!(wit.contains("variant color {"), "got:\n{wit}");
    assert!(wit.contains("red,"), "got:\n{wit}");
    assert!(wit.contains("rgb(tuple<u8, u8, u8>),"), "got:\n{wit}");
}

#[test]
fn free_functions() {
    let wit = generate();
    assert!(
        wit.contains("add: func(a: s32, b: s32) -> s32;"),
        "got:\n{wit}"
    );
    assert!(
        wit.contains("greet: func(name: string) -> string;"),
        "got:\n{wit}"
    );
    assert!(
        wit.contains("midpoint: func(a: borrow<point>, b: borrow<point>) -> point;"),
        "got:\n{wit}"
    );
    assert!(
        wit.contains("parse-point: func(text: string) -> result<point, string>;"),
        "got:\n{wit}"
    );
    assert!(
        wit.contains("fetch-data: async func(url: string) -> string;"),
        "got:\n{wit}"
    );
}

#[test]
fn interfaces_and_world() {
    let wit = generate();
    assert!(wit.contains("interface geometry {"), "got:\n{wit}");
    assert!(wit.contains("interface geometry-utils {"), "got:\n{wit}");
    // Size/Direction are unclaimed by modules -> default interface.
    assert!(wit.contains("interface types {"), "got:\n{wit}");
    assert!(wit.contains("world host {"), "got:\n{wit}");
    assert!(wit.contains("import types;"), "got:\n{wit}");
    assert!(wit.contains("import geometry;"), "got:\n{wit}");
    assert!(wit.contains("import geometry-utils;"), "got:\n{wit}");
}

#[test]
fn constants_as_getters() {
    let wit = generate();
    assert!(wit.contains("pi: func() -> f64;"), "got:\n{wit}");
    assert!(
        wit.contains("/// Constant value: 3.141592653589793"),
        "got:\n{wit}"
    );
    assert!(wit.contains("max-vertices: func() -> s32;"), "got:\n{wit}");
}

#[test]
fn constants_skipped_when_configured() {
    let generator = WitGenerator::new("haphe:demo").with_constant_mode(ConstantMode::Skip);
    let output = haphe::generate(&generator, &REGISTRY).unwrap();
    let wit = String::from_utf8(output.files[0].content.clone()).unwrap();
    assert!(!wit.contains("pi: func"), "got:\n{wit}");
    assert!(!wit.contains("max-vertices"), "got:\n{wit}");
}

#[test]
fn custom_world_and_default_interface() {
    let generator = WitGenerator::new("haphe:demo")
        .with_world("MyHost")
        .with_default_interface("shared");
    let output = haphe::generate(&generator, &REGISTRY).unwrap();
    assert_eq!(output.files[0].path, "wit/my-host.wit");
    let wit = String::from_utf8(output.files[0].content.clone()).unwrap();
    assert!(wit.contains("world my-host {"), "got:\n{wit}");
    assert!(wit.contains("interface shared {"), "got:\n{wit}");
}

#[test]
fn docs_are_emitted() {
    let wit = generate();
    assert!(wit.contains("/// A 2D point."), "got:\n{wit}");
    assert!(
        wit.contains("/// Geometry types and utilities"),
        "got:\n{wit}"
    );
    assert!(wit.contains("/// Adds two integers."), "got:\n{wit}");
}

#[test]
fn invalid_package_name_rejected() {
    for bad in [
        "nocolon",
        "Upper:name",
        "ns:",
        ":name",
        "ns:with_underscore",
    ] {
        let generator = WitGenerator::new(bad);
        let err = haphe::generate(&generator, &REGISTRY).expect_err(bad);
        assert!(
            matches!(
                err,
                haphe::GenerateError::Backend(haphe_wit::WitGenError::InvalidPackageName(_))
            ),
            "expected InvalidPackageName for {bad}, got: {err:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Generics (monomorphization)
// ---------------------------------------------------------------------------

/// A labeled value.
#[derive(Script)]
struct Labeled<T, U> {
    label: String,
    value: T,
    extra: U,
}

/// Tags a labeled string.
#[script]
fn tag(l: Labeled<String, i32>) -> String {
    l.label
}

haphe::registry! {
    pub static GENERIC_REGISTRY = {
        structs: [Labeled<String, i32>, Labeled<bool, u8>],
        modules: [
            mod tags {
                doc: "Tagging",
                functions: [tag],
                types: [Labeled<String, i32>],
            },
        ],
    };
}

fn generate_generic() -> String {
    let output =
        haphe::generate(&WitGenerator::new("haphe:demo"), &GENERIC_REGISTRY).expect("generates");
    String::from_utf8(output.files[0].content.clone()).unwrap()
}

#[test]
fn generic_instantiations_are_monomorphized() {
    let wit = generate_generic();
    assert!(wit.contains("record labeled-string-s32 {"), "got:\n{wit}");
    assert!(wit.contains("record labeled-bool-u8 {"), "got:\n{wit}");
    // Substituted fields.
    assert!(wit.contains("value: string,"), "got:\n{wit}");
    assert!(wit.contains("extra: u8,"), "got:\n{wit}");
    // No erased/general emission.
    assert!(!wit.contains("record labeled {"), "got:\n{wit}");
    // Function references use the mangled name.
    assert!(
        wit.contains("tag: func(l: labeled-string-s32) -> string;"),
        "got:\n{wit}"
    );
}

#[test]
fn generic_instances_carry_marker_comments() {
    let wit = generate_generic();
    assert!(
        wit.contains("/// haphe:generic-instance = labeled<string, s32>"),
        "got:\n{wit}"
    );
    assert!(
        wit.contains("/// haphe:generic-instance = labeled<bool, u8>"),
        "got:\n{wit}"
    );
}

#[test]
fn generic_instances_owned_by_claiming_module() {
    let wit = generate_generic();
    // The `tags` module claims the erased `Labeled` type, so every
    // instantiation of it is emitted inside `interface tags`.
    let tags_pos = wit.find("interface tags {").unwrap();
    assert!(
        wit.find("record labeled-string-s32 {").unwrap() > tags_pos,
        "got:\n{wit}"
    );
    assert!(
        wit.find("record labeled-bool-u8 {").unwrap() > tags_pos,
        "got:\n{wit}"
    );
    // Nothing else is registered, so no default interface is emitted.
    assert!(!wit.contains("interface types {"), "got:\n{wit}");
}

#[test]
fn generation_is_reproducible() {
    assert_eq!(generate_generic(), generate_generic());
}

// ---------------------------------------------------------------------------
// Flags
// ---------------------------------------------------------------------------

/// File permission bits.
#[derive(Script)]
#[script(flags)]
enum Perm {
    Read,
    Write,
    Exec,
}

haphe::registry! {
    pub static FLAGS_REGISTRY = {
        enums: [Perm],
    };
}

#[test]
fn flags_enum_emits_wit_flags() {
    let output =
        haphe::generate(&WitGenerator::new("haphe:demo"), &FLAGS_REGISTRY).expect("generates");
    let wit = String::from_utf8(output.files[0].content.clone()).unwrap();
    assert!(wit.contains("flags perm {"), "got:\n{wit}");
    assert!(wit.contains("read,"), "got:\n{wit}");
    assert!(wit.contains("write,"), "got:\n{wit}");
    assert!(wit.contains("exec,"), "got:\n{wit}");
    assert!(!wit.contains("enum perm"), "got:\n{wit}");
}
