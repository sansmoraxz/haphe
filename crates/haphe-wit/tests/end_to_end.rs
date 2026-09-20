//! Full integration test: derive macros → registry! → `haphe::generate` →
//! WIT text assertions.

#![allow(
    dead_code,
    clippy::approx_constant,
    clippy::needless_pass_by_value,
    clippy::unused_self,
    reason = "fixture shapes are dictated by the bridge surface under test: receivers and owned parameters mirror the declared script signatures, not local call ergonomics"
)]

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
#[derive(Script, PartialEq)]
#[script(traits(PartialEq))]
struct Size {
    width: u32,
    height: u32,
}

/// A buffer resource exercising the full trait-projection table.
#[derive(Script, Clone, Default, PartialEq, Debug, Hash)]
#[script(
    thread_safety = send_sync,
    traits(
        PartialEq,
        Default,
        Debug,
        Hash,
        Add,
        Index(index = i64, output = i64),
        IndexMut(index = i64, output = i64),
        IntoIterator(item = i64)
    ),
    methods
)]
struct Buffer {
    #[script(skip)]
    data: Vec<i64>,
}

impl std::ops::Add for Buffer {
    type Output = Buffer;
    fn add(mut self, rhs: Buffer) -> Buffer {
        self.data.extend(rhs.data);
        self
    }
}

impl std::ops::Index<i64> for Buffer {
    type Output = i64;
    fn index(&self, i: i64) -> &i64 {
        &self.data[i as usize]
    }
}

impl std::ops::IndexMut<i64> for Buffer {
    fn index_mut(&mut self, i: i64) -> &mut i64 {
        &mut self.data[i as usize]
    }
}

impl IntoIterator for Buffer {
    type Item = i64;
    type IntoIter = std::vec::IntoIter<i64>;
    fn into_iter(self) -> Self::IntoIter {
        self.data.into_iter()
    }
}

#[script]
impl Buffer {
    #[script(constructor)]
    fn new() -> Self {
        Buffer { data: Vec::new() }
    }

    fn push(&mut self, v: i64) {
        self.data.push(v);
    }
}

/// A named color.
#[derive(Script)]
enum Color {
    Red,
    Green,
    Blue,
    Rgb(u8, u8, u8),
}

/// A pointer event: a mixed payload enum (unit + single + struct cases).
#[derive(Script)]
enum Event {
    Idle,
    Scroll(f64),
    Move { x: f64, y: f64 },
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

/// Uppercases text, borrowing when it can. The `Borrowed` descriptor lowers
/// as its inner type: WIT values cross by copy, so the lifetime vanishes.
#[script]
fn shout(text: std::borrow::Cow<'_, str>) -> std::borrow::Cow<'_, str> {
    std::borrow::Cow::Owned(text.to_uppercase())
}

/// Midpoint of two points.
#[script]
fn midpoint(a: &Point, b: &Point) -> Point {
    Point {
        x: f64::midpoint(a.x, b.x),
        y: f64::midpoint(a.y, b.y),
    }
}

/// Fetches remote data.
#[script]
async fn fetch_data(url: String) -> String {
    url
}

/// Sums a fixed block of four samples.
#[script]
fn sum_block(samples: [u8; 4]) -> u32 {
    samples.iter().map(|&s| u32::from(s)).sum()
}

/// Parses a point from text.
///
/// Fallible with a described-record ok type: the descriptor (and the WIT
/// text) see `point`; the error crosses as a Host failure with no wire
/// representation.
#[script]
fn parse_point(text: String) -> Result<Point, TextError> {
    let (x, y) = text.split_once(',').ok_or(TextError("bad format".into()))?;
    Ok(Point {
        x: x.trim().parse().map_err(|e| TextError(format!("{e}")))?,
        y: y.trim().parse().map_err(|e| TextError(format!("{e}")))?,
    })
}

/// Host-side notifications.
#[script(foreign, thread_safety = none)]
trait Notifier {
    /// Sends a message to the embedder.
    fn notify(&self, message: String);

    #[script(error_kind = "IoError")]
    fn moved(&self, to: Point) -> Result<u32, NotifyError>;

    async fn flush(&self);
}

struct NotifyError;

impl From<haphe::ForeignError> for NotifyError {
    fn from(_: haphe::ForeignError) -> Self {
        NotifyError
    }
}

// Dispatch shuttles user-defined values as opaque userdata.
impl From<Point> for haphe::ScriptValue {
    fn from(p: Point) -> Self {
        haphe::ScriptValue::UserData(haphe::OpaqueUserData::new(p))
    }
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

haphe::registry! {
    pub static REGISTRY = {
        structs: [Point, Size, Buffer],
        enums: [Color, Direction, Event],
        foreign: [NotifierHandle],
        modules: [
            mod geometry {
                doc: "Geometry types and utilities",
                functions: [add, mul, midpoint, parse_point, fetch_data, sum_block],
                types: [Point, Color],
                constants: [
                    /// The ratio of a circle's circumference to its diameter.
                    PI: f64 = 3.141_592_653_589_793,
                    /// Maximum number of vertices.
                    MAX_VERTICES: i32 = 1024,
                ],
                modules: [
                    mod utils {
                        doc: "Utility helpers",
                        functions: [greet, shout],
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
fn payload_enum_emits_variant_with_record_case() {
    let wit = generate();
    // Struct cases synthesize a record, emitted before the variant.
    assert!(wit.contains("record event-move {"), "got:\n{wit}");
    assert!(wit.contains("x: f64,"), "got:\n{wit}");
    assert!(wit.contains("variant event {"), "got:\n{wit}");
    assert!(wit.contains("idle,"), "got:\n{wit}");
    assert!(wit.contains("scroll(f64),"), "got:\n{wit}");
    assert!(wit.contains("move(event-move),"), "got:\n{wit}");
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
    // `Cow<'a, str>` lowers as `string`: the lifetime is text-neutral.
    assert!(
        wit.contains("shout: func(text: string) -> string;"),
        "got:\n{wit}"
    );
    assert!(
        wit.contains("midpoint: func(a: borrow<point>, b: borrow<point>) -> point;"),
        "got:\n{wit}"
    );
    // Fallible WITHOUT a declared kind: fallibility is explicit in the
    // descriptor (`error_kind` presence used to be a wrong proxy that
    // rendered this signature infallible) — guests see a real `result`.
    assert!(
        wit.contains("parse-point: func(text: string) -> result<point, script-error>;"),
        "got:\n{wit}"
    );
    // The shared error record is defined once in the interface.
    assert!(wit.contains("record script-error {"), "got:\n{wit}");
    assert!(
        wit.contains("kind: option<string>,")
            && wit.contains("message: string,")
            && wit.contains("type-name: string,")
            && wit.contains("chain: list<string>,"),
        "got:\n{wit}"
    );
    assert!(
        wit.contains("fetch-data: async func(url: string) -> string;"),
        "got:\n{wit}"
    );
    // Fixed-size arrays map to WIT 0.3 fixed-length lists.
    assert!(
        wit.contains("sum-block: func(samples: list<u8, 4>) -> u32;"),
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
        structs: [Labeled<String, i32>, Labeled<bool, u8>, Labeled<[u8; 4], bool>],
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
    // Fixed-length list arguments mangle with their length (`list4-u8`).
    assert!(
        wit.contains("record labeled-list4-u8-bool {"),
        "got:\n{wit}"
    );
    assert!(wit.contains("value: list<u8, 4>,"), "got:\n{wit}");
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

// ---------------------------------------------------------------------------
// Streams and futures
// ---------------------------------------------------------------------------

/// Subscribes to a topic.
#[script]
fn subscribe(topic: String) -> haphe::Stream<String> {
    let _ = topic;
    unimplemented!()
}

/// Schedules a rate lookup.
#[script]
fn schedule(fut: haphe::Future<f64>) -> haphe::Future<f64> {
    fut
}

haphe::registry! {
    pub static STREAMS_REGISTRY = {
        structs: [Labeled<haphe::Stream<i32>, u8>],
        modules: [
            mod events {
                doc: "Event streaming",
                functions: [subscribe, schedule],
                types: [Labeled<haphe::Stream<i32>, u8>],
            },
        ],
    };
}

#[test]
fn stream_and_future_types_emit() {
    let output =
        haphe::generate(&WitGenerator::new("haphe:demo"), &STREAMS_REGISTRY).expect("generates");
    let wit = String::from_utf8(output.files[0].content.clone()).unwrap();
    assert!(
        wit.contains("subscribe: func(topic: string) -> stream<string>;"),
        "got:\n{wit}"
    );
    assert!(
        wit.contains("schedule: func(fut: future<f64>) -> future<f64>;"),
        "got:\n{wit}"
    );
    // Mangling of a stream-typed generic argument.
    assert!(
        wit.contains("record labeled-stream-s32-u8 {"),
        "got:\n{wit}"
    );
    assert!(
        wit.contains("/// haphe:generic-instance = labeled<stream-s32, u8>"),
        "got:\n{wit}"
    );
    assert!(wit.contains("value: stream<s32>,"), "got:\n{wit}");
}

// ---------------------------------------------------------------------------
// Foreign interfaces
// ---------------------------------------------------------------------------

#[test]
fn foreign_interface_is_emitted() {
    let wit = generate();
    assert!(
        wit.contains("/// Host-side notifications.\ninterface notifier {"),
        "got:\n{wit}"
    );
    // The `&self` receiver is dropped; params render normally.
    assert!(
        wit.contains("notify: func(message: string);"),
        "got:\n{wit}"
    );
    // Owned `Point` param pulls a `use` from its defining interface. A
    // fallible FOREIGN method carries the same `script-error` err arm as a
    // provided one — the guest's implementation authors the record, lifted
    // host-side into `ForeignFailure`.
    assert!(wit.contains("use geometry.{point};"), "got:\n{wit}");
    assert!(wit.contains("/// Errors: IoError"), "got:\n{wit}");
    assert!(
        wit.contains("moved: func(to: point) -> result<u32, script-error>;"),
        "got:\n{wit}"
    );
    // The record is therefore defined in the foreign interface too.
    let notifier = wit.split("interface notifier {").nth(1).expect("notifier");
    assert!(
        notifier.contains("record script-error {"),
        "got:\n{notifier}"
    );
    assert!(wit.contains("flush: async func();"), "got:\n{wit}");
}

#[test]
fn world_lists_foreign_interfaces_per_perspective() {
    let wit = generate();
    assert!(wit.contains("import geometry;"), "got:\n{wit}");
    assert!(wit.contains("export notifier;"), "got:\n{wit}");
    assert!(!wit.contains("import notifier;"), "got:\n{wit}");

    // With the world targeted by the haphe program itself, the polarity flips.
    let generator =
        WitGenerator::new("haphe:demo").with_world_perspective(haphe_wit::WorldPerspective::Own);
    let output = haphe::generate(&generator, &REGISTRY).unwrap();
    let wit = String::from_utf8(output.files[0].content.clone()).unwrap();
    assert!(wit.contains("export geometry;"), "got:\n{wit}");
    assert!(wit.contains("import notifier;"), "got:\n{wit}");
}

// ---------------------------------------------------------------------------
// Trait projections (interusability)
// ---------------------------------------------------------------------------

#[test]
fn resource_trait_projections_emit() {
    let wit = generate();
    // Point: PartialEq + Display project as members.
    assert!(
        wit.contains("eq: func(other: borrow<point>) -> bool;"),
        "got:\n{wit}"
    );
    assert!(wit.contains("to-string: func() -> string;"), "got:\n{wit}");
    // Buffer: the full table.
    assert!(
        wit.contains("add: func(rhs: borrow<buffer>) -> buffer;"),
        "got:\n{wit}"
    );
    assert!(
        wit.contains("eq: func(other: borrow<buffer>) -> bool;"),
        "got:\n{wit}"
    );
    assert!(
        wit.contains("to-debug-string: func() -> string;"),
        "got:\n{wit}"
    );
    assert!(wit.contains("hash: func() -> u64;"), "got:\n{wit}");
    assert!(wit.contains("at: func(index: s64) -> s64;"), "got:\n{wit}");
    assert!(
        wit.contains("set-at: func(index: s64, value: s64);"),
        "got:\n{wit}"
    );
    assert!(wit.contains("items: func() -> list<s64>;"), "got:\n{wit}");
    assert!(wit.contains("length: func() -> u64;"), "got:\n{wit}");
    assert!(
        wit.contains("default: static func() -> buffer;"),
        "got:\n{wit}"
    );
    // The snapshot semantics are documented in the output.
    assert!(wit.contains("Eager snapshot"), "got:\n{wit}");
}

#[test]
fn record_trait_projections_emit_as_interface_functions() {
    let wit = generate();
    assert!(
        wit.contains("size-eq: func(this: size, other: size) -> bool;"),
        "got:\n{wit}"
    );
}

// ---------------------------------------------------------------------------
// Record properties: a struct whose only callable surface is computed
// properties keeps value semantics — it lowers as a record, its accessors
// projected as interface functions.
// ---------------------------------------------------------------------------

/// A measurement with computed accessors only.
#[derive(Script, Clone)]
#[script(methods)]
struct Meter {
    raw: f64,
}

#[script]
impl Meter {
    #[script(getter)]
    fn level(&self) -> f64 {
        self.raw
    }

    #[script(setter)]
    fn set_level(&mut self, value: f64) {
        self.raw = value;
    }

    #[script(getter)]
    fn doubled(&self) -> f64 {
        self.raw * 2.0
    }
}

haphe::registry! {
    static METER_REGISTRY = {
        structs: [Meter],
        modules: [
            mod meters { types: [Meter] },
        ],
    };
}

#[test]
fn property_only_structs_lower_as_records_with_projected_accessors() {
    let generator = WitGenerator::new("haphe:demo");
    let output = haphe::generate(&generator, &METER_REGISTRY).expect("generation succeeds");
    let wit = String::from_utf8_lossy(&output.files[0].content).to_string();
    assert!(wit.contains("record meter"), "got:\n{wit}");
    assert!(!wit.contains("resource meter"), "got:\n{wit}");
    assert!(
        wit.contains("meter-level: func(this: meter) -> f64;"),
        "got:\n{wit}"
    );
    assert!(
        wit.contains("meter-set-level: func(this: meter, value: f64) -> meter;"),
        "got:\n{wit}"
    );
    // Readonly property: getter only.
    assert!(
        wit.contains("meter-doubled: func(this: meter) -> f64;"),
        "got:\n{wit}"
    );
    assert!(!wit.contains("meter-set-doubled"), "got:\n{wit}");
}

// ---------------------------------------------------------------------------
// Generic self types: a static generic method on a generic resource composes
// the resource instantiation's environment with the method's own.
// ---------------------------------------------------------------------------

/// A generic gauge resource.
#[cfg(feature = "generics")]
#[derive(Script, Clone)]
#[script(thread_safety = send_sync, methods)]
struct Gauge2<T: haphe::FromScript + haphe::IntoScript + haphe::HapheType + Clone + Send + Sync> {
    #[script(skip)]
    v: T,
}

#[cfg(feature = "generics")]
#[script]
impl<T: haphe::FromScript + haphe::IntoScript + haphe::HapheType + Clone + Send + Sync> Gauge2<T> {
    #[script(constructor)]
    fn new(v: T) -> Self {
        Gauge2 { v }
    }

    /// `base` is typed by the SELF parameter, `k` by the method's own.
    #[script(instantiate(f64))]
    fn mix<U>(&self, base: T, k: U) -> U {
        let _ = base;
        k
    }
}

#[cfg(feature = "generics")]
haphe::registry! {
    static GAUGE2_REGISTRY = {
        structs: [Gauge2<i64>],
        modules: [
            mod gauges { types: [Gauge2<i64>] },
        ],
    };
}

#[cfg(feature = "generics")]
#[test]
fn generic_resource_composes_method_instantiation_envs() {
    let generator = WitGenerator::new("haphe:demo");
    let output = haphe::generate(&generator, &GAUGE2_REGISTRY).expect("generation succeeds");
    let wit = String::from_utf8_lossy(&output.files[0].content).to_string();
    assert!(wit.contains("resource gauge2-s64"), "got:\n{wit}");
    // T substitutes from the resource instance, U from the method's own
    // instantiation.
    assert!(
        wit.contains("mix-f64: func(base: s64, k: f64) -> f64;"),
        "got:\n{wit}"
    );
}

// A dyn method on a generic self type: self-parameter positions render
// concretely (pass-through); only the method's own parameter becomes a
// dispatcher variant.
#[cfg(all(feature = "generics", feature = "dyn-generics"))]
#[derive(Script, Clone)]
#[script(thread_safety = send_sync, methods)]
struct Gauge3<T: haphe::FromScript + haphe::IntoScript + haphe::HapheType + Clone + Send + Sync> {
    #[script(skip)]
    v: T,
}

#[cfg(all(feature = "generics", feature = "dyn-generics"))]
#[script]
impl<T: haphe::FromScript + haphe::IntoScript + haphe::HapheType + Clone + Send + Sync> Gauge3<T> {
    #[script(constructor)]
    fn new(v: T) -> Self {
        Gauge3 { v }
    }

    #[script(dyn, instantiate(bool), instantiate(String))]
    fn pick<U>(&self, base: T, flag: U) -> U {
        let _ = base;
        flag
    }
}

#[cfg(all(feature = "generics", feature = "dyn-generics"))]
haphe::registry! {
    static GAUGE3_REGISTRY = {
        structs: [Gauge3<i64>],
        modules: [
            mod gauges3 { types: [Gauge3<i64>] },
        ],
    };
}

#[cfg(all(feature = "generics", feature = "dyn-generics"))]
#[test]
fn generic_self_dyn_dispatcher_passes_self_typed_positions_through() {
    let generator = WitGenerator::new("haphe:demo");
    let output = haphe::generate(&generator, &GAUGE3_REGISTRY).expect("generation succeeds");
    let wit = String::from_utf8_lossy(&output.files[0].content).to_string();
    // The self-typed position is concrete; the method's own is a variant.
    // The identically-shaped result variant dedups onto the flag's.
    assert!(
        wit.contains(
            "pick-dyn: func(base: s64, flag: gauge3-s64-pick-dyn-flag) -> gauge3-s64-pick-dyn-flag;"
        ),
        "got:\n{wit}"
    );
    assert!(
        wit.contains("variant gauge3-s64-pick-dyn-flag"),
        "got:\n{wit}"
    );
    assert!(wit.contains("%bool(bool)"), "got:\n{wit}");
    // The static monomorphs remain, self positions concrete.
    assert!(
        wit.contains("pick-bool: func(base: s64, flag: bool) -> bool;"),
        "got:\n{wit}"
    );
}

// ---------------------------------------------------------------------------
// Receiver-less associated functions: emitted as `static func` members;
// their live dispatch is pinned in tests/runtime.rs. Newtype-typed ones
// register through trait-presence dispatch and render over the carried type.
// ---------------------------------------------------------------------------

/// A transparent bool newtype: crosses as a native boolean.
#[derive(Script, Clone, Copy)]
#[script(transparent)]
struct Woven(bool);

#[derive(Script, Clone)]
#[script(methods)]
struct Fabric {
    n: i64,
}

#[script]
impl Fabric {
    fn origin() -> i64 {
        0
    }

    fn read(&self) -> i64 {
        self.n
    }

    fn flipped(flag: Woven) -> Woven {
        Woven(!flag.0)
    }

    #[script(error_kind = "MakeError")]
    fn checked_flip(flag: Woven) -> Result<Woven, TextError> {
        if flag.0 {
            Ok(Woven(false))
        } else {
            Err(TextError("already off".into()))
        }
    }
}

haphe::registry! {
    static FABRIC_REGISTRY = {
        structs: [Fabric],
        modules: [
            mod fabrics { types: [Fabric] },
        ],
    };
}

#[test]
fn receiverless_associated_fns_emit_as_static_members() {
    let generator = WitGenerator::new("haphe:demo");
    let output = haphe::generate(&generator, &FABRIC_REGISTRY).expect("generation succeeds");
    let wit = String::from_utf8_lossy(&output.files[0].content).to_string();
    assert!(wit.contains("origin: static func() -> s64;"), "got:\n{wit}");
    // Newtype-typed receiver-less fns render over the carried bool; the
    // fallible one is guest-visible.
    assert!(
        wit.contains("flipped: static func(flag: bool) -> bool;"),
        "got:\n{wit}"
    );
    assert!(
        wit.contains("checked-flip: static func(flag: bool) -> result<bool, script-error>;"),
        "got:\n{wit}"
    );
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
