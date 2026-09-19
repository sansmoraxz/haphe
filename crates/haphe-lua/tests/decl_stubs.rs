//! `LuaLS` declaration-stub generation: text assertions over the `---@meta`
//! file, mirroring what the runtime binder exposes.

#![allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    clippy::doc_markdown,
    clippy::trivially_copy_pass_by_ref,
    reason = "fixture shapes are dictated by the bridge surface under test: `#[script]` functions receive OWNED values (the boundary contract), methods keep their declared receivers (`&self` on Copy enums included), and docs name fixture idents verbatim"
)]
#![allow(
    dead_code,
    reason = "fixtures are exercised through their generated descriptors and bridge wrappers, not direct calls"
)]

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
#[allow(
    clippy::unnecessary_wraps,
    reason = "the `Result` IS the surface under test (fallible stub emission)"
)]
#[script(error_kind = "TagError")]
fn tag(entries: std::collections::HashMap<String, i64>) -> Result<String, TextError> {
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
fn unit_enum_emits_luals_enum_table() {
    let s = generate();
    assert!(s.contains("---@enum Color"), "got:\n{s}");
    assert!(s.contains("    Red = \"Red\","), "got:\n{s}");
    assert!(s.contains("    grey = \"grey\","), "got:\n{s}");
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
fn generic_module_function_stubs_each_monomorph() {
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

    // Static generic functions stub one callable per instantiation under
    // its mangled monomorph name, matching the runtime's table entries.
    let output = haphe::generate(&LuaDeclGenerator::new(), &GENERIC_REGISTRY).unwrap();
    let s = String::from_utf8(output.files[0].content.clone()).unwrap();
    assert!(
        s.contains("function util.echo__i64(value) end"),
        "got:\n{s}"
    );
    assert!(s.contains("---@param value integer"), "got:\n{s}");
}

#[cfg(feature = "generics")]
#[test]
fn generic_foreign_methods_stub_per_their_dispatch() {
    /// Converts host values.
    #[script(foreign)]
    pub trait Convert {
        #[script(instantiate(i64), instantiate(String))]
        fn convert<U>(&self, raw: String) -> U;

        #[script(dyn, instantiate(i64), instantiate(String))]
        fn show<U>(&self, value: U) -> String;
    }

    haphe::registry! {
        static FOREIGN_GENERIC_REGISTRY = {
            foreign: [ConvertHandle],
        };
    }

    let output = haphe::generate(&LuaDeclGenerator::new(), &FOREIGN_GENERIC_REGISTRY).unwrap();
    let s = String::from_utf8(output.files[0].content.clone()).unwrap();
    // Static: one field per instantiation under the mangled handler name.
    assert!(
        s.contains("---@field convert__i64 fun(raw: string): integer"),
        "got:\n{s}"
    );
    assert!(
        s.contains("---@field convert__string fun(raw: string): string"),
        "got:\n{s}"
    );
    // Dyn: one plain-named field, the union of substituted signatures.
    assert!(
        s.contains("---@field show fun(value: integer): string|fun(value: string): string"),
        "got:\n{s}"
    );
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

#[test]
fn iterable_and_indexed_types_document_their_protocols() {
    /// Sequence of readings.
    #[derive(Script, Clone)]
    #[script(traits(IntoIterator(item = i64)))]
    struct Readings {
        #[script(skip)]
        values: Vec<i64>,
    }
    impl IntoIterator for Readings {
        type Item = i64;
        type IntoIter = std::vec::IntoIter<i64>;
        fn into_iter(self) -> Self::IntoIter {
            self.values.into_iter()
        }
    }

    /// Keyed scores.
    #[derive(Script, Clone)]
    #[script(traits(IntoIterator(item = (String, i64))))]
    struct Scores {
        #[script(skip)]
        entries: Vec<(String, i64)>,
    }
    impl IntoIterator for Scores {
        type Item = (String, i64);
        type IntoIter = std::vec::IntoIter<(String, i64)>;
        fn into_iter(self) -> Self::IntoIter {
            self.entries.into_iter()
        }
    }

    /// Indexable buffer.
    #[derive(Script, Clone)]
    #[script(traits(Index(index = i64, output = i64), IndexMut(index = i64, output = i64)))]
    struct Buf {
        #[script(skip)]
        data: Vec<i64>,
    }
    impl std::ops::Index<i64> for Buf {
        type Output = i64;
        fn index(&self, i: i64) -> &i64 {
            &self.data[i as usize]
        }
    }
    impl std::ops::IndexMut<i64> for Buf {
        fn index_mut(&mut self, i: i64) -> &mut i64 {
            &mut self.data[i as usize]
        }
    }

    haphe::registry! {
        static ITER_REGISTRY = {
            structs: [Readings, Scores, Buf],
        };
    }

    let output = haphe::generate(&LuaDeclGenerator::new(), &ITER_REGISTRY).unwrap();
    let s = std::str::from_utf8(&output.files[0].content).unwrap();

    assert!(
        s.contains("---Iterable: `pairs(x)` on 5.2+, `x:iter()` everywhere."),
        "got:\n{s}"
    );
    assert!(
        s.contains("---@return fun(): integer, integer"),
        "got:\n{s}"
    );
    assert!(s.contains("function Readings:iter() end"), "got:\n{s}");
    assert!(s.contains("---@return fun(): string, integer"), "got:\n{s}");
    assert!(s.contains("function Scores:iter() end"), "got:\n{s}");
    assert!(s.contains("---@field [integer] integer"), "got:\n{s}");
}

#[test]
fn pow_and_bitwise_operators_annotated() {
    /// A bit mask.
    #[derive(Script, Clone, Copy)]
    #[script(traits(BitAnd, BitOr, BitXor, Shl, Shr, Not, Pow(rhs = f64, output = Self)))]
    struct Bits {
        raw: u32,
    }
    macro_rules! bits_binops {
        ($($t:ident :: $m:ident => $op:tt),*) => {$(
            impl haphe::ops::$t for Bits {
                type Output = Bits;
                fn $m(self, rhs: Bits) -> Bits { Bits { raw: self.raw $op rhs.raw } }
            }
        )*};
    }
    bits_binops!(BitAnd::bitand => &, BitOr::bitor => |, BitXor::bitxor => ^, Shl::shl => <<, Shr::shr => >>);
    impl haphe::ops::Not for Bits {
        type Output = Bits;
        fn not(self) -> Bits {
            Bits { raw: !self.raw }
        }
    }
    impl haphe::ops::Pow<f64> for Bits {
        type Output = Bits;
        fn pow(self, rhs: f64) -> Bits {
            Bits {
                raw: f64::from(self.raw).powf(rhs) as u32,
            }
        }
    }

    haphe::registry! {
        static BITS_REGISTRY = {
            structs: [Bits],
        };
    }

    let output = haphe::generate(&LuaDeclGenerator::new(), &BITS_REGISTRY).unwrap();
    let s = std::str::from_utf8(&output.files[0].content).unwrap();
    assert!(s.contains("---@operator band(Bits): Bits"), "got:\n{s}");
    assert!(s.contains("---@operator bor(Bits): Bits"), "got:\n{s}");
    assert!(s.contains("---@operator bxor(Bits): Bits"), "got:\n{s}");
    assert!(s.contains("---@operator shl(Bits): Bits"), "got:\n{s}");
    assert!(s.contains("---@operator shr(Bits): Bits"), "got:\n{s}");
    assert!(s.contains("---@operator bnot: Bits"), "got:\n{s}");
    assert!(s.contains("---@operator pow(number): Bits"), "got:\n{s}");
}

#[test]
fn idiv_and_fallback_annotations() {
    /// Explicit floor division.
    #[derive(Script, Clone, Copy)]
    #[script(traits(IDiv))]
    struct Floored {
        n: i64,
    }
    impl haphe::ops::IDiv for Floored {
        type Output = Floored;
        fn idiv(self, rhs: Floored) -> Floored {
            Floored {
                n: self.n.div_euclid(rhs.n),
            }
        }
    }

    /// Integer Div only: the runtime answers `//` via the fallback, so the
    /// stub advertises idiv too.
    #[derive(Script, Clone, Copy)]
    #[script(traits(Div(rhs = i64, output = Self)))]
    struct Halver {
        n: i64,
    }
    impl std::ops::Div<i64> for Halver {
        type Output = Halver;
        fn div(self, rhs: i64) -> Halver {
            Halver { n: self.n / rhs }
        }
    }

    /// Float Div: no fallback, no idiv annotation.
    #[derive(Script, Clone, Copy)]
    #[script(traits(Div(rhs = f64, output = Self)))]
    struct Scaler2 {
        v: f64,
    }
    impl std::ops::Div<f64> for Scaler2 {
        type Output = Scaler2;
        fn div(self, rhs: f64) -> Scaler2 {
            Scaler2 { v: self.v / rhs }
        }
    }

    haphe::registry! {
        static IDIV_REGISTRY = {
            structs: [Floored, Halver, Scaler2],
        };
    }

    let output = haphe::generate(&LuaDeclGenerator::new(), &IDIV_REGISTRY).unwrap();
    let s = std::str::from_utf8(&output.files[0].content).unwrap();
    assert!(
        s.contains("---@operator idiv(Floored): Floored"),
        "got:\n{s}"
    );
    assert!(s.contains("---@operator div(integer): Halver"), "got:\n{s}");
    assert!(
        s.contains("---@operator idiv(integer): Halver"),
        "got:\n{s}"
    );
    assert!(!s.contains("idiv(number)"), "got:\n{s}");
}

#[test]
fn mod_annotation_dedupes_with_rem() {
    /// Declares both: Mod wins, one `mod` annotation.
    #[derive(Script, Clone, Copy)]
    #[script(traits(Mod, Rem))]
    struct Cyclic {
        n: i64,
    }
    impl haphe::ops::Mod for Cyclic {
        type Output = Cyclic;
        fn modulo(self, rhs: Cyclic) -> Cyclic {
            Cyclic {
                n: self.n.rem_euclid(rhs.n),
            }
        }
    }
    impl std::ops::Rem for Cyclic {
        type Output = Cyclic;
        fn rem(self, rhs: Cyclic) -> Cyclic {
            Cyclic { n: self.n % rhs.n }
        }
    }

    haphe::registry! {
        static MOD_REGISTRY = {
            structs: [Cyclic],
        };
    }

    let output = haphe::generate(&LuaDeclGenerator::new(), &MOD_REGISTRY).unwrap();
    let s = std::str::from_utf8(&output.files[0].content).unwrap();
    assert_eq!(
        s.matches("---@operator mod(Cyclic): Cyclic").count(),
        1,
        "got:\n{s}"
    );
}

#[test]
fn opaque_primitive_newtype_keeps_named_alias() {
    /// An opaque bool newtype: a distinct declaration-level type.
    #[derive(Script, Clone, Copy)]
    struct Enabled(bool);

    haphe::registry! {
        static NEWTYPE_REGISTRY = {
            type_aliases: [Enabled],
        };
    }

    let output = haphe::generate(&LuaDeclGenerator::new(), &NEWTYPE_REGISTRY).unwrap();
    let s = std::str::from_utf8(&output.files[0].content).unwrap();
    assert!(s.contains("---@alias Enabled boolean"), "got:\n{s}");
}

#[test]
fn callable_types_annotated_as_overloads() {
    /// A callable accumulator.
    #[derive(Script, Clone)]
    struct Acc {
        base: i64,
    }
    // Call is declared without `methods`, so only the trait matters here.
    #[derive(Script, Clone)]
    #[script(traits(Call(args = (i64, i64), output = i64)))]
    struct Summer {
        base: i64,
    }
    impl haphe::ops::Call<(i64, i64)> for Summer {
        type Output = i64;
        fn call(&self, (a, b): (i64, i64)) -> i64 {
            self.base + a + b
        }
    }
    let _ = Acc { base: 0 };

    haphe::registry! {
        static CALL_REGISTRY = {
            structs: [Summer],
        };
    }

    let output = haphe::generate(&LuaDeclGenerator::new(), &CALL_REGISTRY).unwrap();
    let s = std::str::from_utf8(&output.files[0].content).unwrap();
    assert!(
        s.contains("---@overload fun(a1: integer, a2: integer): integer"),
        "got:\n{s}"
    );
}

// The decl generator's capabilities key `async_fns` off the `async`
// feature, so a registry with async methods only validates there.
#[cfg(feature = "async")]
#[test]
fn async_methods_appear_in_stubs() {
    /// Talks to a remote service.
    #[derive(Script, Clone)]
    #[script(thread_safety = none, methods)]
    struct Client {
        host: String,
    }
    #[script]
    impl Client {
        async fn get(&self, path: String) -> String {
            format!("{}/{path}", self.host)
        }
    }

    haphe::registry! {
        static ASYNC_REGISTRY = {
            structs: [Client],
        };
    }

    let output = haphe::generate(&LuaDeclGenerator::new(), &ASYNC_REGISTRY).unwrap();
    let s = std::str::from_utf8(&output.files[0].content).unwrap();
    // Async methods stub like sync ones — the Lua-side calling convention
    // is identical when mlua drives the future.
    assert!(s.contains("function Client:get("), "got:\n{s}");
}

/// A message-only fixture error: `String` itself no longer crosses (host
/// errors must implement `std::error::Error`).
#[derive(Debug)]
struct TextError(String);

impl std::fmt::Display for TextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TextError {}
