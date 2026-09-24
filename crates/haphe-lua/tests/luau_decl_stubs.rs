//! Luau declaration-stub generation: text assertions over the `.d.luau` file,
//! mirroring what the runtime binder exposes in Luau-native type syntax.

#![cfg(feature = "luau-types")]
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
use haphe_lua::{LuauDeclError, LuauDeclGenerator};

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
    let output =
        haphe::generate(&LuauDeclGenerator::new(), &REGISTRY).expect("generation succeeds");
    assert_eq!(output.files.len(), 1);
    assert_eq!(output.files[0].path, "definitions.d.luau");
    String::from_utf8(output.files[0].content.clone()).unwrap()
}

#[test]
fn stub_has_luau_extension_and_is_deterministic() {
    let a = generate();
    let b = generate();
    assert_eq!(a, b, "generation must be byte-deterministic");
    assert!(
        !a.starts_with("---@meta"),
        "Luau stubs must not use LuaLS format"
    );
}

#[test]
fn declare_class_with_fields_and_operators() {
    let s = generate();
    assert!(
        s.contains("-- A 2D point.\ndeclare class Point"),
        "got:\n{s}"
    );
    assert!(s.contains("    x: number\n"), "got:\n{s}");
    assert!(s.contains("    read y: number\n"), "got:\n{s}");
    assert!(
        s.contains("    function __add(self, rhs: Point): Point"),
        "got:\n{s}"
    );
    assert!(
        s.contains("    function __add(self, rhs: number): Point"),
        "got:\n{s}"
    );
    assert!(
        s.contains("    function __unm(self): Point"),
        "got:\n{s}"
    );
    assert!(
        s.contains("    function __tostring(self): string"),
        "got:\n{s}"
    );
    assert!(
        s.contains("    function __concat(self, rhs: string | number): string"),
        "got:\n{s}"
    );
    assert!(s.contains("\nend\n"), "got:\n{s}");
}

#[test]
fn methods_inside_declare_class() {
    let s = generate();
    assert!(
        s.contains("    function distance_to(self, other: Point): number"),
        "got:\n{s}"
    );
}

#[test]
fn constructors_in_module_type_table() {
    let s = generate();
    assert!(s.contains("Point: {"), "got:\n{s}");
    assert!(
        s.contains("new: (x: number, y: number) -> Point,"),
        "got:\n{s}"
    );
}

#[test]
fn unit_enum_emits_string_literal_union() {
    let s = generate();
    assert!(
        s.contains("type Color = \"Red\" | \"grey\""),
        "got:\n{s}"
    );
}

#[test]
fn module_declared_as_table_type() {
    let s = generate();
    assert!(
        s.contains("-- Geometry utilities\ndeclare geometry: {"),
        "got:\n{s}"
    );
}

#[test]
fn module_functions_use_arrow_syntax() {
    let s = generate();
    assert!(
        s.contains("pick: (name: string, fallback: number?) -> {number},"),
        "got:\n{s}"
    );
    // Map + Result-return + error_kind doc.
    assert!(
        s.contains("tag: (entries: {[string]: number}) -> string,"),
        "got:\n{s}"
    );
    assert!(s.contains("-- Errors raise (TagError)."), "got:\n{s}");
    // Callback param.
    assert!(
        s.contains("cb: (a1: number) -> boolean"),
        "got:\n{s}"
    );
}

#[test]
fn constants_are_typed_fields() {
    let s = generate();
    assert!(
        s.contains("-- Maximum number of vertices."),
        "got:\n{s}"
    );
    assert!(s.contains("MAX_VERTICES: number,"), "got:\n{s}");
}

#[test]
fn nested_submodule() {
    let s = generate();
    assert!(s.contains("inner: {"), "got:\n{s}");
    assert!(
        s.contains("pick: (name: string, fallback: number?) -> {number},"),
        "got:\n{s}"
    );
}

#[test]
fn foreign_interface_as_type_table() {
    let s = generate();
    assert!(s.contains("type Hooks = {"), "got:\n{s}");
    assert!(
        s.contains("progress: (done: number, total: number) -> (),"),
        "got:\n{s}"
    );
    assert!(
        s.contains("fetch: (url: string) -> string,"),
        "got:\n{s}"
    );
    assert!(s.contains("-- May raise (IoError)."), "got:\n{s}");
}

#[test]
fn enum_case_table_in_module() {
    let s = generate();
    assert!(s.contains("Color: {"), "got:\n{s}");
    assert!(s.contains("Red: Color,"), "got:\n{s}");
    assert!(s.contains("grey: Color,"), "got:\n{s}");
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

    let output = haphe::generate(&LuauDeclGenerator::new(), &GENERIC_REGISTRY).unwrap();
    let s = String::from_utf8(output.files[0].content.clone()).unwrap();
    assert!(
        s.contains("echo__i64: (value: number) -> number,"),
        "got:\n{s}"
    );
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

    let output = haphe::generate(&LuauDeclGenerator::new(), &FOREIGN_GENERIC_REGISTRY).unwrap();
    let s = String::from_utf8(output.files[0].content.clone()).unwrap();
    // Static: one field per instantiation under the mangled handler name.
    assert!(
        s.contains("convert__i64: (raw: string) -> number,"),
        "got:\n{s}"
    );
    assert!(
        s.contains("convert__string: (raw: string) -> string,"),
        "got:\n{s}"
    );
    // Dyn: intersection of substituted signatures.
    assert!(
        s.contains("show: (value: number) -> string & (value: string) -> string,"),
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

    let err = haphe::generate(&LuauDeclGenerator::new(), &TWIN_REGISTRY).unwrap_err();
    assert!(
        matches!(
            err,
            haphe::GenerateError::Backend(LuauDeclError::DuplicateTypeName { ref name })
                if name == "Twin"
        ),
        "got: {err:?}"
    );
}

#[test]
fn iterable_types_declare_iter_metamethod() {
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

    haphe::registry! {
        static ITER_REGISTRY = {
            structs: [Readings, Scores],
        };
    }

    let output = haphe::generate(&LuauDeclGenerator::new(), &ITER_REGISTRY).unwrap();
    let s = std::str::from_utf8(&output.files[0].content).unwrap();

    assert!(s.contains("function __iter(self)"), "got:\n{s}");
    assert!(s.contains("function iter(self)"), "got:\n{s}");
}

#[test]
fn indexed_types_declare_index_metamethods() {
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
        static INDEX_REGISTRY = {
            structs: [Buf],
        };
    }

    let output = haphe::generate(&LuauDeclGenerator::new(), &INDEX_REGISTRY).unwrap();
    let s = std::str::from_utf8(&output.files[0].content).unwrap();
    assert!(
        s.contains("function __index(self, key: number): number"),
        "got:\n{s}"
    );
    assert!(
        s.contains("function __newindex(self, key: number, value: number)"),
        "got:\n{s}"
    );
}

#[test]
fn callable_types_declare_call_metamethod() {
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

    haphe::registry! {
        static CALL_REGISTRY = {
            structs: [Summer],
        };
    }

    let output = haphe::generate(&LuauDeclGenerator::new(), &CALL_REGISTRY).unwrap();
    let s = std::str::from_utf8(&output.files[0].content).unwrap();
    assert!(
        s.contains("function __call(self, a1: number, a2: number): number"),
        "got:\n{s}"
    );
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

    /// Integer Div only: the runtime answers `//` via the fallback.
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

    let output = haphe::generate(&LuauDeclGenerator::new(), &IDIV_REGISTRY).unwrap();
    let s = std::str::from_utf8(&output.files[0].content).unwrap();
    assert!(
        s.contains("function __idiv(self, rhs: Floored): Floored"),
        "got:\n{s}"
    );
    assert!(
        s.contains("function __div(self, rhs: number): Halver"),
        "got:\n{s}"
    );
    assert!(
        s.contains("function __idiv(self, rhs: number): Halver"),
        "got:\n{s}"
    );
    // Float Div should NOT produce idiv.
    let scaler_section = s.split("declare class Scaler2").nth(1).unwrap();
    let scaler_end = scaler_section.find("end").unwrap();
    let scaler_block = &scaler_section[..scaler_end];
    assert!(!scaler_block.contains("__idiv"), "got:\n{s}");
}

#[test]
fn opaque_primitive_newtype_keeps_named_alias() {
    /// An opaque bool newtype.
    #[derive(Script, Clone, Copy)]
    struct Enabled(bool);

    haphe::registry! {
        static NEWTYPE_REGISTRY = {
            type_aliases: [Enabled],
        };
    }

    let output = haphe::generate(&LuauDeclGenerator::new(), &NEWTYPE_REGISTRY).unwrap();
    let s = std::str::from_utf8(&output.files[0].content).unwrap();
    assert!(s.contains("type Enabled = boolean"), "got:\n{s}");
}

#[test]
fn bitwise_operators_omitted_for_luau() {
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

    let output = haphe::generate(&LuauDeclGenerator::new(), &BITS_REGISTRY).unwrap();
    let s = std::str::from_utf8(&output.files[0].content).unwrap();
    // Pow should be present.
    assert!(
        s.contains("function __pow(self, rhs: number): Bits"),
        "got:\n{s}"
    );
    // Bitwise ops should NOT be present.
    assert!(!s.contains("__band"), "bitwise ops should be omitted:\n{s}");
    assert!(!s.contains("__bor"), "bitwise ops should be omitted:\n{s}");
    assert!(!s.contains("__bxor"), "bitwise ops should be omitted:\n{s}");
    assert!(!s.contains("__shl"), "bitwise ops should be omitted:\n{s}");
    assert!(!s.contains("__shr"), "bitwise ops should be omitted:\n{s}");
    assert!(!s.contains("__bnot"), "bitwise ops should be omitted:\n{s}");
}

#[test]
fn mod_annotation_dedupes_with_rem() {
    /// Declares both: Mod wins, one `__mod` declaration.
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

    let output = haphe::generate(&LuauDeclGenerator::new(), &MOD_REGISTRY).unwrap();
    let s = std::str::from_utf8(&output.files[0].content).unwrap();
    assert_eq!(
        s.matches("function __mod(self, rhs: Cyclic): Cyclic").count(),
        1,
        "got:\n{s}"
    );
}

/// A message-only fixture error.
#[derive(Debug)]
struct TextError(String);

impl std::fmt::Display for TextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TextError {}
