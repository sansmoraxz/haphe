//! Round-trip validation: the generated document must parse and resolve
//! through `wit-parser`, the reference implementation of the WIT grammar.

#![allow(dead_code, clippy::approx_constant)]

use haphe::{Script, script};
use haphe_wit::WitGenerator;

/// A 2D point.
#[derive(Script, Clone)]
#[script(traits(Clone), methods)]
struct Point {
    x: f64,
    #[script(readonly)]
    y: f64,
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

/// A plain size record.
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

/// Scales a size.
#[script]
fn scale(size: Size, factor: f64) -> Size {
    Size {
        width: (size.width as f64 * factor) as u32,
        height: (size.height as f64 * factor) as u32,
    }
}

/// Midpoint of two points.
#[script]
fn midpoint(a: &Point, b: &Point) -> Point {
    Point {
        x: (a.x + b.x) / 2.0,
        y: (a.y + b.y) / 2.0,
    }
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

haphe::registry! {
    pub static REGISTRY = {
        structs: [Point, Size],
        enums: [Color],
        modules: [
            mod geometry {
                doc: "Geometry types and utilities",
                functions: [midpoint, parse_point],
                types: [Point],
                constants: [
                    /// Circle constant.
                    PI: f64 = 3.141592653589793,
                ],
                modules: [
                    mod shapes {
                        doc: "Shape helpers",
                        functions: [scale],
                        types: [Size],
                    },
                ],
            },
        ],
    };
}

#[test]
fn generated_wit_resolves() {
    let generator = WitGenerator::new("haphe:demo").with_version("0.1.0");
    let output = haphe::generate(&generator, &REGISTRY).expect("generation succeeds");
    let wit = std::str::from_utf8(&output.files[0].content).unwrap();

    let mut resolve = wit_parser::Resolve::new();
    resolve
        .push_str("host.wit", wit)
        .unwrap_or_else(|e| panic!("generated WIT failed to resolve: {e}\n---\n{wit}"));
}
