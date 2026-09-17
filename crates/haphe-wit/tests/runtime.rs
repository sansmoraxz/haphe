//! Runtime binding tests: bind the registry into a wasmtime component
//! linker and execute real guest components against it.
//!
//! Building a genuine guest would require the wit-bindgen toolchain, so the
//! guests here are hand-written component WAT. Each follows the same shape:
//!
//! 1. `import "haphe:demo/geometry"` — declare the host instance (only the
//!    functions this guest uses; instantiation type-checks them against what
//!    `WasmBinder` defined in the linker).
//! 2. `canon lower` — turn the imported component-level function into a core
//!    wasm function.
//! 3. a `core module` whose exported `run` calls that import.
//! 4. `canon lift` — re-export core `run` as the component-level `run` that
//!    the test invokes.

#![allow(dead_code, clippy::approx_constant)]

use haphe::{RuntimeBinder, Script, script};
use haphe_wit::{WasmBindError, WasmBinder, WitGenerator};
use wasmtime::Engine;
use wasmtime::component::{Component, Linker, Val};

/// A 2D point.
#[derive(Script, Clone, Default, PartialEq, Debug)]
#[script(thread_safety = send_sync, traits(Clone, PartialEq, Display, Default), methods)]
struct Point {
    x: f64,
    y: f64,
}

impl std::fmt::Display for Point {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {})", self.x, self.y)
    }
}

// Resource values cross fn signatures as opaque userdata.
impl From<Point> for haphe::ScriptValue {
    fn from(p: Point) -> Self {
        haphe::ScriptValue::UserData(haphe::OpaqueUserData::new(p))
    }
}

impl haphe::FromScript for Point {
    fn from_script(v: haphe::ScriptValue) -> Result<Self, haphe::ScriptConvertError> {
        match v {
            haphe::ScriptValue::UserData(ud) => {
                ud.downcast_clone::<Point>()
                    .ok_or(haphe::ScriptConvertError {
                        expected: "Point",
                        got: "a different userdata",
                    })
            }
            other => Err(haphe::ScriptConvertError {
                expected: "Point",
                got: other.variant_name(),
            }),
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

    fn scale(&mut self, k: f64) {
        self.x *= k;
        self.y *= k;
    }

    fn take_x(self) -> f64 {
        self.x
    }

    async fn xy_sum(&self) -> f64 {
        self.x + self.y
    }

    async fn nudge(&mut self, dx: f64) {
        self.x += dx;
    }
}

/// A second resource type, for cross-type handle checks.
#[derive(Script, Clone)]
#[script(thread_safety = send_sync, methods)]
struct Counter {
    n: i64,
}

#[script]
impl Counter {
    #[script(constructor)]
    fn new() -> Self {
        Counter { n: 0 }
    }

    /// Seeds a counter asynchronously.
    #[script(constructor)]
    async fn seeded(n: i64) -> Self {
        Counter { n }
    }

    fn bump(&mut self) -> i64 {
        self.n += 1;
        self.n
    }

    #[script(getter)]
    fn doubled(&self) -> i64 {
        self.n * 2
    }

    #[script(setter)]
    fn set_doubled(&mut self, value: i64) {
        self.n = value / 2;
    }

    #[script(getter)]
    async fn lagged(&self) -> i64 {
        self.n + 100
    }

    #[script(setter = "lagged")]
    async fn set_lagged(&mut self, value: i64) {
        self.n = value - 100;
    }
}

/// Adds two integers.
#[script]
fn add(a: i32, b: i32) -> i32 {
    a + b
}

/// Midpoint of two points (resource params and return through a free fn).
#[script]
fn midpoint(a: &Point, b: &Point) -> Point {
    Point {
        x: (a.x + b.x) / 2.0,
        y: (a.y + b.y) / 2.0,
    }
}

/// Fetches a rate asynchronously.
#[script]
async fn fetch_rate(id: i32) -> f64 {
    id as f64
}

struct Empty;

impl haphe::futures_core::Stream for Empty {
    type Item = i32;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<i32>> {
        std::task::Poll::Ready(None)
    }
}

/// Streams up to `n` counts.
#[script]
fn counts(n: i32) -> haphe::Stream<i32> {
    let _ = n;
    haphe::Stream::new(Empty)
}

/// Resolves to `x` eventually.
#[script]
fn delayed(x: f64) -> haphe::Future<f64> {
    haphe::Future::new(async move { x })
}

haphe::registry! {
    pub static REGISTRY = {
        structs: [Point, Counter],
        modules: [
            mod geometry {
                doc: "Geometry utilities",
                functions: [add, midpoint, fetch_rate, counts, delayed],
                types: [Point, Counter],
                constants: [
                    /// Circle constant.
                    PI: f64 = 3.141592653589793,
                ],
            },
        ],
    };
}

fn binder() -> WasmBinder<()> {
    WasmBinder::new(WitGenerator::new("haphe:demo"))
}

/// Instantiates `guest_wat` against `linker` and calls its exported `run`
/// function (no arguments, one result).
fn run_guest<T>(
    engine: &Engine,
    linker: &Linker<T>,
    data: T,
    guest_wat: &str,
) -> Result<Val, wasmtime::Error> {
    let component = Component::new(engine, wat::parse_str(guest_wat)?)?;
    let mut store = wasmtime::Store::new(engine, data);
    let instance = linker.instantiate(&mut store, &component)?;
    let run = instance
        .get_func(&mut store, "run")
        .expect("guest exports `run`");
    let mut results = [Val::Bool(false)];
    run.call(&mut store, &[], &mut results)?;
    Ok(results.into_iter().next().unwrap())
}

/// Guest that calls the host's `pi` constant getter and returns the value.
const PI_GUEST: &str = r#"
(component
  (import "haphe:demo/geometry" (instance $geo
    (export "pi" (func (result f64)))
  ))
  (core func $pi (canon lower (func $geo "pi")))
  (core module $m
    (import "geo" "pi" (func $pi (result f64)))
    (func (export "run") (result f64) (call $pi))
  )
  (core instance $mi (instantiate $m
    (with "geo" (instance (export "pi" (func $pi))))
  ))
  (func (export "run") (result f64) (canon lift (core func $mi "run")))
)
"#;

/// Guest that calls the host's `add` function (a trap-stub) with 1 and 2.
const ADD_GUEST: &str = r#"
(component
  (import "haphe:demo/geometry" (instance $geo
    (export "add" (func (param "a" s32) (param "b" s32) (result s32)))
  ))
  (core func $add (canon lower (func $geo "add")))
  (core module $m
    (import "geo" "add" (func $add (param i32 i32) (result i32)))
    (func (export "run") (result i32) (call $add (i32.const 1) (i32.const 2)))
  )
  (core instance $mi (instantiate $m
    (with "geo" (instance (export "add" (func $add))))
  ))
  (func (export "run") (result s32) (canon lift (core func $mi "run")))
)
"#;

/// Guest that constructs a `point` resource. It also declares the
/// `distance-to` method import, so instantiation verifies the binder defined
/// the whole resource surface (`point` type, constructor, method) — but only
/// the constructor is called, since it traps before any handle exists.
const POINT_GUEST: &str = r#"
(component
  (import "haphe:demo/geometry" (instance $geo
    (export "point" (type $point (sub resource)))
    (export "[constructor]point" (func (param "x" f64) (param "y" f64) (result (own $point))))
    (export "[method]point.distance-to" (func (param "self" (borrow $point)) (param "other" (borrow $point)) (result f64)))
  ))
  (core func $ctor (canon lower (func $geo "[constructor]point")))
  (core module $m
    (import "geo" "ctor" (func $ctor (param f64 f64) (result i32)))
    (func (export "run") (result f64)
      (drop (call $ctor (f64.const 1) (f64.const 2)))
      (f64.const 0))
  )
  (core instance $mi (instantiate $m
    (with "geo" (instance (export "ctor" (func $ctor))))
  ))
  (func (export "run") (result f64) (canon lift (core func $mi "run")))
)
"#;

/// Guest that calls `fetch-rate`, declared `async` in the registry. At the
/// component level asyncness is a canonical (lift/lower) option, not part of
/// the function type, so a guest may lower it synchronously.
const ASYNC_GUEST: &str = r#"
(component
  (import "haphe:demo/geometry" (instance $geo
    (export "fetch-rate" (func (param "id" s32) (result f64)))
  ))
  (core func $fetch (canon lower (func $geo "fetch-rate")))
  (core module $m
    (import "geo" "fetch-rate" (func $fetch (param i32) (result f64)))
    (func (export "run") (result f64) (call $fetch (i32.const 7)))
  )
  (core instance $mi (instantiate $m
    (with "geo" (instance (export "fetch-rate" (func $fetch))))
  ))
  (func (export "run") (result f64) (canon lift (core func $mi "run")))
)
"#;

#[test]
fn binder_reports_language_and_accepts_registry() {
    let binder = binder();
    assert_eq!(binder.language_name(), "wasm");
    let validated = REGISTRY.validate().unwrap();
    binder
        .capabilities()
        .check(&validated)
        .expect("registry is compatible");
}

/// The bound constant getter is a real host function: the guest calls
/// `haphe:demo/geometry.pi()` and receives the registry's constant value.
#[test]
fn guest_receives_bound_constant_value() {
    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    haphe::bind(&binder(), &REGISTRY, &mut linker).expect("binding succeeds");

    let result = run_guest(&engine, &linker, (), PI_GUEST).expect("guest runs");
    match result {
        Val::Float64(pi) => assert!((pi - 3.141592653589793).abs() < 1e-15),
        other => panic!("expected f64, got: {other:?}"),
    }
}

/// Negative control for the test above: without `haphe::bind`, the same
/// guest cannot even instantiate — proving the import is satisfied by the
/// binder and not by wasmtime defaults.
#[test]
fn guest_fails_to_instantiate_against_empty_linker() {
    let engine = Engine::default();
    let empty_linker = Linker::new(&engine);

    let err = run_guest(&engine, &empty_linker, (), PI_GUEST).expect_err("missing import");
    let msg = format!("{err:?}");
    assert!(msg.contains("haphe:demo/geometry"), "got: {msg}");
}

/// Functions are bound at stub depth: the definition exists (the guest
/// links and instantiates) but calling it traps with a clear message.
#[test]
fn stub_function_links_but_traps_when_called() {
    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    haphe::bind(&binder(), &REGISTRY, &mut linker).expect("binding succeeds");

    let err = run_guest(&engine, &linker, (), ADD_GUEST).expect_err("stub should trap");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("haphe-wit: `add`: not registered"),
        "got: {msg}"
    );
}

/// The resource surface (type, constructor, `distance-to` method) is defined
/// in the linker — the guest's typed imports instantiate — and the
/// constructor stub traps when called.
#[test]
fn resource_surface_links_and_constructor_traps() {
    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    haphe::bind(&binder(), &REGISTRY, &mut linker).expect("binding succeeds");

    let err = run_guest(&engine, &linker, (), POINT_GUEST).expect_err("ctor stub should trap");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("declared but not registered; call `register_type`"),
        "got: {msg}"
    );
}

/// An `async fn` in the registry binds like any other function at this
/// depth: the definition links and its stub traps when called.
#[test]
fn async_function_links_and_stub_traps() {
    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    haphe::bind(&binder(), &REGISTRY, &mut linker).expect("binding succeeds");

    let err = run_guest(&engine, &linker, (), ASYNC_GUEST).expect_err("stub should trap");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("not registered; call the binder's `register_*` method"),
        "got: {msg}"
    );
}

/// A registered async free function dispatches live: the boxed future is
/// driven on the calling thread and its result crosses back.
#[test]
fn async_free_function_dispatches_live() {
    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    let mut b = binder();
    b.register_fn::<fetch_rate>().unwrap();
    haphe::bind(&b, &REGISTRY, &mut linker).expect("binding succeeds");

    let result = run_guest(&engine, &linker, (), ASYNC_GUEST).expect("guest runs");
    assert!(
        matches!(result, Val::Float64(v) if v == 7.0),
        "got: {result:?}"
    );
}

/// Guest importing `counts`, whose result is `stream<s32>` — at the core
/// level the stream is an `i32` handle. Instantiation type-checks the
/// stream-shaped signature against the binder's definition.
const STREAM_GUEST: &str = r#"
(component
  (import "haphe:demo/geometry" (instance $geo
    (export "counts" (func (param "n" s32) (result (stream s32))))
  ))
  (core func $counts (canon lower (func $geo "counts")))
  (core module $m
    (import "geo" "counts" (func $counts (param i32) (result i32)))
    (func (export "run") (result f64)
      (drop (call $counts (i32.const 3)))
      (f64.const 0))
  )
  (core instance $mi (instantiate $m
    (with "geo" (instance (export "counts" (func $counts))))
  ))
  (func (export "run") (result f64) (canon lift (core func $mi "run")))
)
"#;

/// Guest importing `delayed`, whose result is `future<f64>` — an `i32`
/// handle at the core level, like streams.
const FUTURE_GUEST: &str = r#"
(component
  (import "haphe:demo/geometry" (instance $geo
    (export "delayed" (func (param "x" f64) (result (future f64))))
  ))
  (core func $delayed (canon lower (func $geo "delayed")))
  (core module $m
    (import "geo" "delayed" (func $delayed (param f64) (result i32)))
    (func (export "run") (result f64)
      (drop (call $delayed (f64.const 1.5)))
      (f64.const 0))
  )
  (core instance $mi (instantiate $m
    (with "geo" (instance (export "delayed" (func $delayed))))
  ))
  (func (export "run") (result f64) (canon lift (core func $mi "run")))
)
"#;

/// A `stream<T>`-returning function is defined in the linker with the right
/// shape (the guest's typed import instantiates) and its stub traps.
#[test]
fn stream_function_links_and_stub_traps() {
    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    haphe::bind(&binder(), &REGISTRY, &mut linker).expect("binding succeeds");

    let err = run_guest(&engine, &linker, (), STREAM_GUEST).expect_err("stub should trap");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("haphe-wit: `counts`: not registered"),
        "got: {msg}"
    );
}

/// A `future<T>`-returning function is defined in the linker with the right
/// shape and its stub traps.
#[test]
fn future_function_links_and_stub_traps() {
    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    haphe::bind(&binder(), &REGISTRY, &mut linker).expect("binding succeeds");

    let err = run_guest(&engine, &linker, (), FUTURE_GUEST).expect_err("stub should trap");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("haphe-wit: `delayed`: not registered"),
        "got: {msg}"
    );
}

/// Guest importing both a WASI 0.3 interface (`wasi:clocks/system-clock`)
/// and the haphe interface. `now` returns `record instant { seconds: s64,
/// nanoseconds: u32 }`, which lowers through a caller-provided return
/// pointer (hence the guest memory in `canon lower`). `run` returns -1
/// unless the reported seconds are after 2020-09-13 (epoch 1600000000) —
/// i.e. the host handed back real system time — otherwise the host's `pi`.
const WASI_GUEST: &str = r#"
(component
  (import "wasi:clocks/system-clock@0.3.0" (instance $clock
    (type $instant-def (record (field "seconds" s64) (field "nanoseconds" u32)))
    (export "instant" (type $instant (eq $instant-def)))
    (export "now" (func (result $instant)))
  ))
  (import "haphe:demo/geometry" (instance $geo
    (export "pi" (func (result f64)))
  ))
  (core module $libc (memory (export "mem") 1))
  (core instance $libc (instantiate $libc))
  (core func $now (canon lower (func $clock "now") (memory (core memory $libc "mem"))))
  (core func $pi (canon lower (func $geo "pi")))
  (core module $m
    (import "libc" "mem" (memory 1))
    (import "host" "now" (func $now (param i32)))
    (import "host" "pi" (func $pi (result f64)))
    (func (export "run") (result f64)
      (call $now (i32.const 0))
      (if (result f64) (i64.gt_s (i64.load (i32.const 0)) (i64.const 1600000000))
        (then (call $pi))
        (else (f64.const -1))))
  )
  (core instance $mi (instantiate $m
    (with "libc" (instance $libc))
    (with "host" (instance
      (export "now" (func $now))
      (export "pi" (func $pi))))
  ))
  (func (export "run") (result f64) (canon lift (core func $mi "run")))
)
"#;

/// WASI wiring is not implicit: `haphe::bind` defines only the registry's
/// own interfaces, so a guest importing `wasi:*` fails to instantiate until
/// the host composes `wasmtime-wasi` into the same linker. Once composed,
/// both the WASI call and the haphe call execute for real.
#[test]
fn wasi_interfaces_compose_with_binder_in_one_linker() {
    use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

    struct Host {
        ctx: WasiCtx,
        table: wasmtime::component::ResourceTable,
    }

    impl WasiView for Host {
        fn ctx(&mut self) -> WasiCtxView<'_> {
            WasiCtxView {
                ctx: &mut self.ctx,
                table: &mut self.table,
            }
        }
    }

    let host = || Host {
        ctx: WasiCtx::builder().build(),
        table: wasmtime::component::ResourceTable::new(),
    };

    let engine = Engine::default();

    // Without wasmtime-wasi in the linker the WASI import is unsatisfied.
    let mut haphe_only: Linker<Host> = Linker::new(&engine);
    haphe::bind(
        &WasmBinder::<Host>::new(WitGenerator::new("haphe:demo")),
        &REGISTRY,
        &mut haphe_only,
    )
    .expect("binding succeeds");
    let err = run_guest(&engine, &haphe_only, host(), WASI_GUEST).expect_err("wasi import missing");
    assert!(
        format!("{err:?}").contains("wasi:clocks/system-clock"),
        "got: {err:?}"
    );

    // Composed: wasmtime-wasi (WASI 0.3) and the haphe binder share one linker.
    let mut linker: Linker<Host> = Linker::new(&engine);
    wasmtime_wasi::p3::add_to_linker(&mut linker).expect("wasi links");
    haphe::bind(
        &WasmBinder::<Host>::new(WitGenerator::new("haphe:demo")),
        &REGISTRY,
        &mut linker,
    )
    .expect("binding succeeds");

    let result = run_guest(&engine, &linker, host(), WASI_GUEST).expect("guest runs");
    match result {
        Val::Float64(pi) => assert!((pi - 3.141592653589793).abs() < 1e-15, "got: {pi}"),
        other => panic!("expected f64, got: {other:?}"),
    }
}

/// Planning errors surface through `haphe::bind` the same way they do
/// through `haphe::generate`.
#[test]
fn name_collision_propagates() {
    const fn empty_struct(
        id: &'static str,
        name: &'static str,
    ) -> haphe::StructDescriptor<'static> {
        haphe::StructDescriptor {
            id: haphe::TypeId::new(id),
            name,
            doc: None,
            fields: &[],
            methods: &[],
            constructors: &[],
            properties: &[],
            trait_impls: &[],
            thread_safety: haphe::ThreadSafety::SEND_SYNC,
            generic_params: &[],
        }
    }
    // Both names kebab-case to `my-type`.
    static STRUCTS: [haphe::StructDescriptor; 2] = [
        empty_struct("t::MyType", "MyType"),
        empty_struct("t::my_type", "my_type"),
    ];
    static REGISTRY_BAD: haphe::TypeRegistry =
        haphe::TypeRegistry::new(&STRUCTS, &[], &[], &[], &[], &[]);

    let engine = Engine::default();
    let mut linker = Linker::<()>::new(&engine);
    match haphe::bind(&binder(), &REGISTRY_BAD, &mut linker) {
        Err(haphe::BindError::Bind(WasmBindError::Gen(
            haphe_wit::WitGenError::NameCollision { .. },
        ))) => {}
        other => panic!("expected NameCollision, got: {other:?}"),
    }
}

/// Guest importing a monomorphized generic resource (`Holder<f64>` →
/// `holder-f64`) and calling its stubbed getter.
const GENERIC_GUEST: &str = r#"
(component
  (import "haphe:demo/types" (instance $t
    (export "holder-f64" (type $h (sub resource)))
    (export "[method]holder-f64.get" (func (param "self" (borrow $h)) (result f64)))
  ))
  (core module $m
    (func (export "run") (result f64) (f64.const 0))
  )
  (core instance $mi (instantiate $m))
  (func (export "run") (result f64) (canon lift (core func $mi "run")))
)
"#;

/// Generic instantiations are bound under their mangled names: the guest's
/// typed imports (resource + method) link against the binder's definitions.
#[test]
fn generic_instance_resource_links() {
    static T_PARAM: haphe::TypeDescriptor = haphe::TypeDescriptor::GenericParam("T");
    static GET: [haphe::FunctionDescriptor; 1] = [haphe::FunctionDescriptor {
        name: "get",
        doc: None,
        receiver: Some(haphe::Receiver::Ref),
        generic_params: &[],
        instantiations: &[],
        dispatch: haphe::Dispatch::Static,
        params: &[],
        return_type: &T_PARAM,
        return_ownership: haphe::Ownership::Owned,
        is_async: false,
        error_kind: None,
    }];
    static STRUCTS: [haphe::StructDescriptor; 1] = [haphe::StructDescriptor {
        id: haphe::TypeId::new("test::Holder"),
        name: "Holder",
        doc: None,
        fields: &[],
        methods: &GET,
        constructors: &[],
        properties: &[],
        trait_impls: &[],
        thread_safety: haphe::ThreadSafety::SEND_SYNC,
        generic_params: &[haphe::GenericParam {
            name: "T",
            bounds: &[],
            default: None,
        }],
    }];
    static F64_ARGS: [haphe::TypeDescriptor; 1] =
        [haphe::TypeDescriptor::Primitive(haphe::PrimitiveType::F64)];
    static INSTANTIATIONS: [haphe::InstantiationDescriptor; 1] = [haphe::InstantiationDescriptor {
        id: haphe::TypeId::new("test::Holder"),
        args: &F64_ARGS,
    }];
    static GENERIC_REGISTRY: haphe::TypeRegistry =
        haphe::TypeRegistry::new(&STRUCTS, &[], &[], &[], &INSTANTIATIONS, &[]);

    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    haphe::bind(&binder(), &GENERIC_REGISTRY, &mut linker).expect("binding succeeds");

    // Instantiation succeeding proves `holder-f64` and its method stub are
    // defined with compatible shapes.
    let result = run_guest(&engine, &linker, (), GENERIC_GUEST).expect("guest instantiates");
    assert!(matches!(result, Val::Float64(_)));
}

// ---------------------------------------------------------------------------
// Foreign interfaces: Rust calls into guest exports
// ---------------------------------------------------------------------------

/// Host-side math the Rust program calls out to.
#[script(foreign, thread_safety = none)]
trait HostMath {
    fn add(&self, a: i32, b: i32) -> i32;
}

/// Guest exporting the `host-math` foreign interface.
const MATH_GUEST: &str = r#"
(component
  (core module $m
    (func (export "add") (param i32 i32) (result i32)
      (i32.add (local.get 0) (local.get 1))))
  (core instance $mi (instantiate $m))
  (func $add (param "a" s32) (param "b" s32) (result s32)
    (canon lift (core func $mi "add")))
  (instance $i (export "add" (func $add)))
  (export "haphe:demo/host-math" (instance $i))
)
"#;

/// Same surface, but the implementation traps.
const TRAPPING_MATH_GUEST: &str = r#"
(component
  (core module $m
    (func (export "add") (param i32 i32) (result i32)
      (unreachable)))
  (core instance $mi (instantiate $m))
  (func $add (param "a" s32) (param "b" s32) (result s32)
    (canon lift (core func $mi "add")))
  (instance $i (export "add" (func $add)))
  (export "haphe:demo/host-math" (instance $i))
)
"#;

fn foreign_handle_from_wat<H: haphe::ScriptForeign + haphe::ForeignHandle>(
    guest_wat: &str,
) -> Result<H, WasmBindError> {
    let engine = Engine::default();
    let component = Component::new(&engine, wat::parse_str(guest_wat).unwrap()).unwrap();
    let linker: Linker<()> = Linker::new(&engine);
    let mut store = wasmtime::Store::new(&engine, ());
    let instance = linker.instantiate(&mut store, &component).unwrap();
    let store = std::sync::Arc::new(std::sync::Mutex::new(store));
    haphe_wit::foreign_handle("haphe:demo", store, &instance)
}

#[test]
fn foreign_handle_dispatches_into_guest_export() {
    let math: HostMathHandle = foreign_handle_from_wat(MATH_GUEST).unwrap();
    assert_eq!(math.add(2, 3), 5);
    assert_eq!(math.add(-10, 4), -6);
}

#[test]
fn missing_foreign_instance_is_reported() {
    let Err(err) = foreign_handle_from_wat::<HostMathHandle>("(component)") else {
        panic!("expected an error");
    };
    assert!(
        matches!(err, WasmBindError::MissingForeignInstance { ref interface }
            if interface == "haphe:demo/host-math"),
        "got: {err:?}"
    );
}

#[test]
fn missing_foreign_function_is_reported() {
    const EMPTY_INSTANCE_GUEST: &str = r#"
(component
  (core module $m
    (func (export "other") (result i32) (i32.const 0)))
  (core instance $mi (instantiate $m))
  (func $other (result s32) (canon lift (core func $mi "other")))
  (instance $i (export "other" (func $other)))
  (export "haphe:demo/host-math" (instance $i))
)
"#;
    let Err(err) = foreign_handle_from_wat::<HostMathHandle>(EMPTY_INSTANCE_GUEST) else {
        panic!("expected an error");
    };
    assert!(
        matches!(err, WasmBindError::MissingForeignExport { ref function, .. }
            if function == "add"),
        "got: {err:?}"
    );
}

#[test]
#[should_panic(expected = "foreign function `add` failed")]
fn trapping_guest_export_panics_through_non_result_method() {
    let math: HostMathHandle = foreign_handle_from_wat(TRAPPING_MATH_GUEST).unwrap();
    math.add(1, 1);
}

// ---------------------------------------------------------------------------
// Generic foreign interfaces (`generics` naming extension)
// ---------------------------------------------------------------------------

/// Host-side scaling, generic over the result type.
#[cfg(feature = "generics")]
#[script(foreign, thread_safety = none)]
trait Scaler<T> {
    fn scale(&self, by: i64) -> T;
}

#[cfg(feature = "generics")]
const SCALER_GUEST: &str = r#"
(component
  (core module $m
    (func (export "scale") (param i64) (result i64)
      (i64.mul (local.get 0) (i64.const 2))))
  (core instance $mi (instantiate $m))
  (func $scale (param "by" s64) (result s64)
    (canon lift (core func $mi "scale")))
  (instance $i (export "scale" (func $scale)))
  (export "haphe:demo/scaler-s64" (instance $i))
)
"#;

/// The handle's type arguments resolve the monomorphized instance export
/// deterministically (same mangling the generator emits).
#[cfg(feature = "generics")]
#[test]
fn generic_foreign_handle_resolves_monomorphized_export() {
    let scaler: ScalerHandle<i64> = foreign_handle_from_wat(SCALER_GUEST).unwrap();
    assert_eq!(scaler.scale(21), 42);
}

// ---------------------------------------------------------------------------
// Foreign dispatch modes: static routes to mangled monomorphs, `dyn` to one
// erased variant import
// ---------------------------------------------------------------------------

/// Method-level STATIC generics: the host provides one export per declared
/// instantiation under the mangled member name.
#[cfg(feature = "generics")]
#[script(foreign, thread_safety = none)]
trait Parser {
    #[script(instantiate(i64))]
    fn parse<T>(&self, raw: i64) -> T;
}

#[cfg(feature = "generics")]
const PARSER_GUEST: &str = r#"
(component
  (core module $m
    (func (export "parse") (param i64) (result i64) (local.get 0)))
  (core instance $mi (instantiate $m))
  (func $parse (param "raw" s64) (result s64)
    (canon lift (core func $mi "parse")))
  (instance $i (export "parse-s64" (func $parse)))
  (export "haphe:demo/parser" (instance $i))
)
"#;

/// A declared instantiation dispatches through its mangled export.
#[cfg(feature = "generics")]
#[test]
fn static_foreign_method_generics_route_to_mangles() {
    let parser: ParserHandle = foreign_handle_from_wat(PARSER_GUEST).unwrap();
    let n: i64 = parser.parse(41);
    assert_eq!(n, 41);
}

/// A call whose type arguments match no DECLARED instantiation fails
/// descriptively, naming the declared set — never a silent miss.
#[cfg(feature = "generics")]
#[test]
#[should_panic(
    expected = "no declared instantiation matches these type arguments; declared: [s64]"
)]
fn static_foreign_method_generics_reject_undeclared_type_args() {
    let parser: ParserHandle = foreign_handle_from_wat(PARSER_GUEST).unwrap();
    let _: f64 = parser.parse(41);
}

/// Method-level `dyn` generics: ERASED addressing — the host provides ONE
/// export under the plain name, generic slots crossing as case variants.
#[cfg(feature = "dyn-generics")]
#[script(foreign, thread_safety = none)]
trait DynCodec {
    #[script(dyn, instantiate(i64), instantiate(f64))]
    fn bump<T>(&self, value: T) -> T;
}

/// One guest export `bump` serving both cases: s64 payloads gain 1,
/// f64 payloads gain 0.5. Variant param flattens to (disc: i32, payload:
/// i64-join); the variant result returns indirectly (disc u8 at +0, payload
/// at +8).
#[cfg(feature = "dyn-generics")]
const DYN_CODEC_GUEST: &str = r#"
(component
  (type $vt (variant (case "s64" s64) (case "f64" f64)))
  (core module $m
    (memory (export "mem") 1)
    (func (export "bump") (param i32 i64) (result i32)
      (if (i32.eqz (local.get 0))
        (then (i64.store offset=8 (i32.const 16) (i64.add (local.get 1) (i64.const 1))))
        (else (f64.store offset=8 (i32.const 16)
          (f64.add (f64.reinterpret_i64 (local.get 1)) (f64.const 0.5)))))
      (i32.store8 (i32.const 16) (local.get 0))
      (i32.const 16))
  )
  (core instance $mi (instantiate $m))
  (func $bump (param "value" $vt) (result $vt)
    (canon lift (core func $mi "bump") (memory (core memory $mi "mem"))))
  (instance $i
    (export "bump-dyn-value" (type $vt))
    (export "bump" (func $bump)))
  (export "haphe:demo/dyn-codec" (instance $i))
)
"#;

/// Both cases route through the single erased export; the case tag carries
/// the instantiation, so type-ambiguous payloads never guess.
#[cfg(feature = "dyn-generics")]
#[test]
fn dyn_foreign_method_dispatches_through_one_erased_export() {
    let codec: DynCodecHandle = foreign_handle_from_wat(DYN_CODEC_GUEST).unwrap();
    let n: i64 = codec.bump(41);
    assert_eq!(n, 42);
    let f: f64 = codec.bump(2.0f64);
    assert_eq!(f, 2.5);
}

/// An undeclared instantiation fails descriptively, naming the declared
/// case set.
#[cfg(feature = "dyn-generics")]
#[test]
#[should_panic(
    expected = "no declared instantiation matches these type arguments; declared: [s64, f64]"
)]
fn dyn_foreign_method_rejects_undeclared_type_args() {
    let codec: DynCodecHandle = foreign_handle_from_wat(DYN_CODEC_GUEST).unwrap();
    let _: String = codec.bump("x".to_string());
}

// ---------------------------------------------------------------------------
// Composite values through the foreign caller
// ---------------------------------------------------------------------------

/// A plain data pair (WIT record).
#[derive(Script, Clone, Debug, PartialEq)]
#[script(thread_safety = send_sync, traits(PartialEq))]
struct Pair {
    a: f64,
    b: f64,
}

impl From<Pair> for haphe::ScriptValue {
    fn from(p: Pair) -> Self {
        haphe::ScriptValue::Map(vec![
            ("a".to_string(), haphe::ScriptValue::F64(p.a)),
            ("b".to_string(), haphe::ScriptValue::F64(p.b)),
        ])
    }
}

impl haphe::FromScript for Pair {
    fn from_script(v: haphe::ScriptValue) -> Result<Self, haphe::ScriptConvertError> {
        let err = haphe::ScriptConvertError {
            expected: "Pair",
            got: v.variant_name(),
        };
        let haphe::ScriptValue::Map(pairs) = v else {
            return Err(err);
        };
        let field = |name: &str| {
            pairs
                .iter()
                .find(|(k, _)| k == name)
                .and_then(|(_, v)| match v {
                    haphe::ScriptValue::F64(n) => Some(*n),
                    haphe::ScriptValue::I64(n) => Some(*n as f64),
                    _ => None,
                })
                .ok_or(haphe::ScriptConvertError {
                    expected: "Pair field",
                    got: "missing or non-numeric field",
                })
        };
        Ok(Pair {
            a: field("a")?,
            b: field("b")?,
        })
    }
}

/// A unit enum (WIT enum) with a numeric repr: cases carry discriminants.
#[derive(Script, Debug, PartialEq)]
#[repr(u8)]
enum Fruit {
    Apple,
    DragonFruit,
}

/// Host-side composite operations.
#[script(foreign, thread_safety = none)]
trait Composite {
    fn sum_pair(&self, p: Pair) -> f64;
    fn pick(&self, n: i64) -> Fruit;
    fn rate(&self, f: Fruit) -> i64;
}

const COMPOSITE_GUEST: &str = r#"
(component
  (core module $m
    (func (export "sum-pair") (param f64 f64) (result f64)
      (f64.add (local.get 0) (local.get 1)))
    (func (export "pick") (param i64) (result i32)
      (i32.wrap_i64 (local.get 0)))
    (func (export "rate") (param i32) (result i64)
      (i64.extend_i32_u (local.get 0))))
  (core instance $mi (instantiate $m))
  (type $pair (record (field "a" f64) (field "b" f64)))
  (type $fruit (enum "apple" "dragon-fruit"))
  (func $sum-pair (param "p" $pair) (result f64)
    (canon lift (core func $mi "sum-pair")))
  (func $pick (param "n" s64) (result $fruit)
    (canon lift (core func $mi "pick")))
  (func $rate (param "f" $fruit) (result s64)
    (canon lift (core func $mi "rate")))
  (instance $i
    (export "pair" (type $pair))
    (export "fruit" (type $fruit))
    (export "sum-pair" (func $sum-pair))
    (export "pick" (func $pick))
    (export "rate" (func $rate)))
  (export "haphe:demo/composite" (instance $i))
)
"#;

haphe::registry! {
    pub static COMPOSITE_REGISTRY = {
        structs: [Pair],
        enums: [Fruit],
        foreign: [CompositeHandle],
    };
}

#[test]
fn composite_values_cross_the_foreign_boundary() {
    // Enum returns need the registry-aware caller: the guest's kebab case
    // names are translated back to the declared variant names it carries.
    let engine = Engine::default();
    let component = Component::new(&engine, wat::parse_str(COMPOSITE_GUEST).unwrap()).unwrap();
    let linker: Linker<()> = Linker::new(&engine);
    let mut store = wasmtime::Store::new(&engine, ());
    let instance = linker.instantiate(&mut store, &component).unwrap();
    let store = std::sync::Arc::new(std::sync::Mutex::new(store));
    let validated = COMPOSITE_REGISTRY.validate().unwrap();
    let comp: CompositeHandle =
        haphe_wit::foreign_handle_in("haphe:demo", store, &instance, &validated).unwrap();
    // Record param (ScriptValue::Map -> Val::Record).
    assert_eq!(comp.sum_pair(Pair { a: 2.0, b: 3.0 }), 5.0);
    // Enum return: the guest's "dragon-fruit" is translated to the declared
    // `DragonFruit`, which the derive-generated FromScript matches exactly.
    assert_eq!(comp.pick(1), Fruit::DragonFruit);
    assert_eq!(comp.pick(0), Fruit::Apple);
    // Enum param: the declared case lowers to the guest's spelling via the
    // crate's kebab conversion.
    assert_eq!(comp.rate(Fruit::Apple), 0);
    assert_eq!(comp.rate(Fruit::DragonFruit), 1);
}

#[test]
fn numeric_enum_discriminants_cross_the_boundary() {
    let engine = Engine::default();
    let component = Component::new(&engine, wat::parse_str(COMPOSITE_GUEST).unwrap()).unwrap();
    let linker: Linker<()> = Linker::new(&engine);
    let mut store = wasmtime::Store::new(&engine, ());
    let instance = linker.instantiate(&mut store, &component).unwrap();
    let store = std::sync::Arc::new(std::sync::Mutex::new(store));
    let validated = COMPOSITE_REGISTRY.validate().unwrap();
    let caller = haphe_wit::foreign_caller_in(
        "haphe:demo",
        store,
        &instance,
        &<CompositeHandle as haphe::ScriptForeign>::DESCRIPTOR,
        &[],
        &validated,
    )
    .unwrap();

    // Lifted enum returns carry the recorded discriminant.
    let out = caller
        .call("pick", &[], &[haphe::ScriptValue::I64(1)])
        .unwrap();
    assert!(matches!(
        out,
        haphe::ScriptValue::Enum { ref case, discriminant: Some(1) } if case == "DragonFruit"
    ));

    // A plain integer lowers into a numeric enum parameter.
    let out = caller
        .call("rate", &[], &[haphe::ScriptValue::I64(0)])
        .unwrap();
    assert!(matches!(out, haphe::ScriptValue::I64(0)));
    // Unknown discriminants error instead of guessing.
    let err = caller
        .call("rate", &[], &[haphe::ScriptValue::I64(9)])
        .unwrap_err();
    assert!(format!("{err}").contains("discriminant"), "got: {err}");
}

// ---------------------------------------------------------------------------
// Enum companion methods (live dispatch)
// ---------------------------------------------------------------------------

/// A rated fruit grade with a method, dispatched as a companion function.
#[derive(Script, Debug, PartialEq, Clone, Copy)]
#[script(methods, thread_safety = send_sync)]
enum Grade {
    Poor,
    Fine,
}

#[script]
impl Grade {
    fn score(&self, bonus: i64) -> i64 {
        let base = match self {
            Grade::Poor => 1,
            Grade::Fine => 10,
        };
        base + bonus
    }
}

haphe::registry! {
    pub static GRADE_REGISTRY = {
        enums: [Grade],
    };
}

/// Calls `grade-score(fine, 5)` -> 15.
const GRADE_GUEST: &str = r#"
(component
  (type $grade-def (enum "poor" "fine"))
  (import "haphe:demo/types" (instance $t
    (export "grade" (type $grade (eq $grade-def)))
    (export "grade-score" (func (param "self" $grade) (param "bonus" s64) (result s64)))
  ))
  (core func $score (canon lower (func $t "grade-score")))
  (core module $m
    (import "t" "score" (func $score (param i32 i64) (result i64)))
    (func (export "run") (result i64)
      (call $score (i32.const 1) (i64.const 5)))
  )
  (core instance $mi (instantiate $m
    (with "t" (instance (export "score" (func $score))))
  ))
  (func (export "run") (result s64) (canon lift (core func $mi "run")))
)
"#;

#[test]
fn enum_companion_method_dispatches_live() {
    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    let mut binder = WasmBinder::new(WitGenerator::new("haphe:demo"));
    binder.register_enum::<Grade>().unwrap();
    haphe::bind(&binder, &GRADE_REGISTRY, &mut linker).expect("binding succeeds");

    let result = run_guest(&engine, &linker, (), GRADE_GUEST).expect("guest runs");
    assert!(matches!(result, Val::S64(15)), "got: {result:?}");
}

#[test]
fn unregistered_enum_companion_traps_descriptively() {
    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    let binder = WasmBinder::new(WitGenerator::new("haphe:demo"));
    haphe::bind(&binder, &GRADE_REGISTRY, &mut linker).expect("binding succeeds");

    let err = run_guest(&engine, &linker, (), GRADE_GUEST).expect_err("stub traps");
    assert!(format!("{err:?}").contains("register_enum"), "got: {err:?}");
}

// ---------------------------------------------------------------------------
// True async dispatch
// ---------------------------------------------------------------------------

/// Async host math (dispatching into the same `host-math` guest surface).
#[script(foreign, thread_safety = none, rename = "HostMath")]
trait AsyncMath {
    async fn add(&self, a: i32, b: i32) -> i32;
}

fn block_on<F: Future>(mut fut: F) -> F::Output {
    let mut fut = unsafe { std::pin::Pin::new_unchecked(&mut fut) };
    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);
    loop {
        if let std::task::Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
            return out;
        }
    }
}

#[test]
fn async_caller_dispatches_via_call_async() {
    let engine = Engine::default();
    let component = Component::new(&engine, wat::parse_str(MATH_GUEST).unwrap()).unwrap();
    let linker: Linker<()> = Linker::new(&engine);
    let mut store = wasmtime::Store::new(&engine, ());
    let instance = block_on(linker.instantiate_async(&mut store, &component)).unwrap();
    let store = std::sync::Arc::new(std::sync::Mutex::new(store));
    let math: AsyncMathHandle =
        haphe_wit::foreign_handle_async("haphe:demo", store, &instance).unwrap();
    assert_eq!(block_on(math.add(20, 22)), 42);
}

/// A sync method through an async caller refuses with a clear error.
#[test]
#[should_panic(expected = "asynchronously")]
fn sync_dispatch_through_async_caller_is_refused() {
    let engine = Engine::default();
    let component = Component::new(&engine, wat::parse_str(MATH_GUEST).unwrap()).unwrap();
    let linker: Linker<()> = Linker::new(&engine);
    let mut store = wasmtime::Store::new(&engine, ());
    let instance = block_on(linker.instantiate_async(&mut store, &component)).unwrap();
    let store = std::sync::Arc::new(std::sync::Mutex::new(store));
    let math: HostMathHandle =
        haphe_wit::foreign_handle_async("haphe:demo", store, &instance).unwrap();
    math.add(1, 1);
}

// ---------------------------------------------------------------------------
// Registry-aware generic resolution (`generics` feature)
// ---------------------------------------------------------------------------

/// Tags a value with a score.
#[cfg(feature = "generics")]
#[script(foreign, thread_safety = none)]
trait Tagger<T> {
    fn tag(&self, value: T) -> f64;
}

#[cfg(feature = "generics")]
haphe::registry! {
    pub static GENERIC_FOREIGN_REGISTRY = {
        structs: [Pair],
        foreign: [TaggerHandle<Pair>],
    };
}

#[cfg(feature = "generics")]
const TAGGER_GUEST: &str = r#"
(component
  (core module $m
    (func (export "tag") (param f64 f64) (result f64)
      (f64.add (local.get 0) (local.get 1))))
  (core instance $mi (instantiate $m))
  (type $pair (record (field "a" f64) (field "b" f64)))
  (func $tag (param "value" $pair) (result f64)
    (canon lift (core func $mi "tag")))
  (instance $i
    (export "pair" (type $pair))
    (export "tag" (func $tag)))
  (export "haphe:demo/tagger-pair" (instance $i))
)
"#;

#[cfg(feature = "generics")]
#[test]
fn registered_type_args_resolve_via_registry_aware_mangling() {
    let engine = Engine::default();
    let component = Component::new(&engine, wat::parse_str(TAGGER_GUEST).unwrap()).unwrap();
    let linker: Linker<()> = Linker::new(&engine);
    let mut store = wasmtime::Store::new(&engine, ());
    let instance = linker.instantiate(&mut store, &component).unwrap();
    let store = std::sync::Arc::new(std::sync::Mutex::new(store));

    // The plain mangler cannot name registered types.
    let err =
        haphe_wit::foreign_handle::<TaggerHandle<Pair>, _>("haphe:demo", store.clone(), &instance)
            .err()
            .expect("plain mangling refuses registered type args");
    assert!(
        err.to_string().contains("not supported at runtime"),
        "got: {err}"
    );

    // Registry-aware mangling resolves `tagger-pair` and dispatches.
    let validated = GENERIC_FOREIGN_REGISTRY.validate().unwrap();
    let tagger: TaggerHandle<Pair> =
        haphe_wit::foreign_handle_in("haphe:demo", store, &instance, &validated).unwrap();
    assert_eq!(tagger.tag(Pair { a: 1.5, b: 2.5 }), 4.0);
}

/// Two descriptor functions must never resolve to one guest export: a
/// hand-written `echo_s64` next to a monomorphized `echo<i64>` collides at
/// caller construction, before any guest lookup.
#[cfg(feature = "generics")]
#[test]
fn colliding_export_names_are_rejected_at_caller_construction() {
    use haphe::{
        ForeignInterfaceDescriptor, FunctionDescriptor, GenericParam, Ownership, Receiver,
        ThreadSafety, TypeDescriptor, TypeId,
    };

    const S64: TypeDescriptor<'static> = TypeDescriptor::Primitive(haphe::PrimitiveType::I64);
    const T: TypeDescriptor<'static> = TypeDescriptor::GenericParam("T");
    static T_PARAM: [GenericParam<'static>; 1] = [GenericParam {
        name: "T",
        bounds: &[],
        default: None,
    }];
    static INSTS: [&[TypeDescriptor<'static>]; 1] = [&[S64]];
    const fn desc_fn(
        name: &'static str,
        generic_params: &'static [GenericParam<'static>],
        instantiations: &'static [&'static [TypeDescriptor<'static>]],
        return_type: &'static TypeDescriptor<'static>,
    ) -> FunctionDescriptor<'static> {
        FunctionDescriptor {
            name,
            doc: None,
            receiver: Some(Receiver::Ref),
            generic_params,
            instantiations,
            dispatch: haphe::Dispatch::Static,
            params: &[],
            return_type,
            return_ownership: Ownership::Owned,
            is_async: false,
            error_kind: None,
        }
    }
    static FNS: [FunctionDescriptor<'static>; 2] = [
        desc_fn("echo_s64", &[], &[], &S64),
        desc_fn("echo", &T_PARAM, &INSTS, &T),
    ];
    static DESC: ForeignInterfaceDescriptor<'static> = ForeignInterfaceDescriptor {
        id: TypeId::new("Echoes"),
        name: "Echoes",
        doc: None,
        generic_params: &[],
        functions: &FNS,
        thread_safety: ThreadSafety::NONE,
    };

    let engine = Engine::default();
    let component = Component::new(&engine, wat::parse_str("(component)").unwrap()).unwrap();
    let linker: Linker<()> = Linker::new(&engine);
    let mut store = wasmtime::Store::new(&engine, ());
    let instance = linker.instantiate(&mut store, &component).unwrap();
    let store = std::sync::Arc::new(std::sync::Mutex::new(store));
    let Err(err) = haphe_wit::foreign_caller("haphe:demo", store, &instance, &DESC, &[]) else {
        panic!("expected a name collision error");
    };
    assert!(
        matches!(
            &err,
            WasmBindError::Gen(haphe_wit::WitGenError::NameCollision { kebab, .. })
                if kebab == "echo-s64"
        ),
        "got: {err:?}"
    );
}

/// The package address may embed a version (`ns:pkg@v`), which becomes part
/// of the instance export name; malformed addresses are rejected up front.
#[test]
fn package_address_forms() {
    let engine = Engine::default();
    let component = Component::new(&engine, wat::parse_str("(component)").unwrap()).unwrap();
    let linker: Linker<()> = Linker::new(&engine);
    let mut store = wasmtime::Store::new(&engine, ());
    let instance = linker.instantiate(&mut store, &component).unwrap();
    let store = std::sync::Arc::new(std::sync::Mutex::new(store));

    let Err(err) = haphe_wit::foreign_handle::<HostMathHandle, _>(
        "haphe:demo@1.2.3",
        store.clone(),
        &instance,
    ) else {
        panic!("expected a missing-instance error");
    };
    assert!(
        matches!(&err, WasmBindError::MissingForeignInstance { interface }
            if interface == "haphe:demo/host-math@1.2.3"),
        "got: {err:?}"
    );

    let Err(err) =
        haphe_wit::foreign_handle::<HostMathHandle, _>("Not A Package", store, &instance)
    else {
        panic!("expected an invalid-address error");
    };
    assert!(
        matches!(
            &err,
            WasmBindError::Gen(haphe_wit::WitGenError::InvalidPackageName(_))
        ),
        "got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Registry-level instantiations: guest stub parity (`generics` feature)
// ---------------------------------------------------------------------------

/// Relays a value; no fn-site `instantiate` attrs — the registry declares
/// the concrete uses.
#[cfg(feature = "generics")]
#[script]
fn relay<T>(value: T) -> T {
    value
}

#[cfg(feature = "generics")]
haphe::registry! {
    static RELAY_REGISTRY = {
        modules: [
            mod pipes { functions: [relay<i64>] },
        ],
    };
}

/// Guest importing the registry-level monomorph under the same mangled name
/// the generator emits.
#[cfg(feature = "generics")]
const RELAY_GUEST: &str = r#"
(component
  (import "haphe:demo/pipes" (instance $p
    (export "relay-s64" (func (param "value" s64) (result s64)))
  ))
  (core func $relay (canon lower (func $p "relay-s64")))
  (core module $m
    (import "p" "relay-s64" (func $relay (param i64) (result i64)))
    (func (export "run") (result i64) (call $relay (i64.const 7)))
  )
  (core instance $mi (instantiate $m
    (with "p" (instance (export "relay-s64" (func $relay))))
  ))
  (func (export "run") (result s64) (canon lift (core func $mi "run")))
)
"#;

/// The binder defines a stub for a monomorph declared only at the registry
/// level: the guest links and instantiates (parity with the generated WIT),
/// and calling it traps at stub depth.
#[cfg(feature = "generics")]
#[test]
fn registry_level_monomorph_stub_links_but_traps() {
    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    haphe::bind(&binder(), &RELAY_REGISTRY, &mut linker).expect("binding succeeds");

    let err = run_guest(&engine, &linker, (), RELAY_GUEST).expect_err("stub should trap");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("haphe-wit: `relay-s64`: not registered"),
        "got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Live provided-direction dispatch
// ---------------------------------------------------------------------------

/// A binder with the geometry registry's types and functions registered.
fn live_binder() -> WasmBinder<()> {
    let mut b = WasmBinder::new(WitGenerator::new("haphe:demo"));
    b.register_type::<Point>().unwrap();
    b.register_type::<Counter>().unwrap();
    b.register_fn::<add>().unwrap();
    b.register_fn::<midpoint>().unwrap();
    b
}

#[test]
fn duplicate_registration_is_rejected() {
    let mut b = WasmBinder::<()>::new(WitGenerator::new("haphe:demo"));
    b.register_type::<Point>().unwrap();
    let Err(err) = b.register_type::<Point>() else {
        panic!("expected a duplicate-registration error");
    };
    assert!(
        matches!(err, WasmBindError::DuplicateRegistration { .. }),
        "got: {err:?}"
    );
}

/// The registered `add` executes for real: 1 + 2 = 3.
#[test]
fn registered_function_dispatches_live() {
    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    haphe::bind(&live_binder(), &REGISTRY, &mut linker).expect("binding succeeds");

    let result = run_guest(&engine, &linker, (), ADD_GUEST).expect("guest runs");
    assert!(matches!(result, Val::S32(3)), "got: {result:?}");
}

/// Full resource lifecycle: construct, borrow-method with a second handle,
/// field set/get, `&mut self` mutation, owned-receiver consumption.
/// d(0,3 -> 4,0) = 5; set-x(p1, 10); scale(p1, 2) -> x = 20; take-x(p2) = 4;
/// total = 5 + 20 + 4 = 29.
const POINT_LIFE_GUEST: &str = r#"
(component
  (import "haphe:demo/geometry" (instance $geo
    (export "point" (type $point (sub resource)))
    (export "[constructor]point" (func (param "x" f64) (param "y" f64) (result (own $point))))
    (export "[method]point.distance-to" (func (param "self" (borrow $point)) (param "other" (borrow $point)) (result f64)))
    (export "[method]point.x" (func (param "self" (borrow $point)) (result f64)))
    (export "[method]point.set-x" (func (param "self" (borrow $point)) (param "value" f64)))
    (export "[method]point.scale" (func (param "self" (borrow $point)) (param "k" f64)))
    (export "[static]point.take-x" (func (param "this" (own $point)) (result f64)))
  ))
  (core func $ctor (canon lower (func $geo "[constructor]point")))
  (core func $dist (canon lower (func $geo "[method]point.distance-to")))
  (core func $getx (canon lower (func $geo "[method]point.x")))
  (core func $setx (canon lower (func $geo "[method]point.set-x")))
  (core func $scale (canon lower (func $geo "[method]point.scale")))
  (core func $takex (canon lower (func $geo "[static]point.take-x")))
  (core module $m
    (import "geo" "ctor" (func $ctor (param f64 f64) (result i32)))
    (import "geo" "dist" (func $dist (param i32 i32) (result f64)))
    (import "geo" "getx" (func $getx (param i32) (result f64)))
    (import "geo" "setx" (func $setx (param i32 f64)))
    (import "geo" "scale" (func $scale (param i32 f64)))
    (import "geo" "takex" (func $takex (param i32) (result f64)))
    (func (export "run") (result f64) (local $p1 i32) (local $p2 i32) (local $acc f64)
      (local.set $p1 (call $ctor (f64.const 0) (f64.const 3)))
      (local.set $p2 (call $ctor (f64.const 4) (f64.const 0)))
      (local.set $acc (call $dist (local.get $p1) (local.get $p2)))
      (call $setx (local.get $p1) (f64.const 10))
      (call $scale (local.get $p1) (f64.const 2))
      (local.set $acc (f64.add (local.get $acc) (call $getx (local.get $p1))))
      (local.set $acc (f64.add (local.get $acc) (call $takex (local.get $p2))))
      (local.get $acc))
  )
  (core instance $mi (instantiate $m
    (with "geo" (instance
      (export "ctor" (func $ctor))
      (export "dist" (func $dist))
      (export "getx" (func $getx))
      (export "setx" (func $setx))
      (export "scale" (func $scale))
      (export "takex" (func $takex))))
  ))
  (func (export "run") (result f64) (canon lift (core func $mi "run")))
)
"#;

#[test]
fn resource_lifecycle_dispatches_live() {
    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    haphe::bind(&live_binder(), &REGISTRY, &mut linker).expect("binding succeeds");

    let result = run_guest(&engine, &linker, (), POINT_LIFE_GUEST).expect("guest runs");
    match result {
        Val::Float64(v) => assert!((v - 29.0).abs() < 1e-12, "got: {v}"),
        other => panic!("expected f64, got: {other:?}"),
    }
}

/// Async methods dispatch through the on-thread executor; `&mut self`
/// mutation persists in place. nudge(p, 9): x = 10; xy-sum = 10 + 2 = 12.
const ASYNC_METHOD_GUEST: &str = r#"
(component
  (import "haphe:demo/geometry" (instance $geo
    (export "point" (type $point (sub resource)))
    (export "[constructor]point" (func (param "x" f64) (param "y" f64) (result (own $point))))
    (export "[method]point.xy-sum" (func (param "self" (borrow $point)) (result f64)))
    (export "[method]point.nudge" (func (param "self" (borrow $point)) (param "dx" f64)))
  ))
  (core func $ctor (canon lower (func $geo "[constructor]point")))
  (core func $sum (canon lower (func $geo "[method]point.xy-sum")))
  (core func $nudge (canon lower (func $geo "[method]point.nudge")))
  (core module $m
    (import "geo" "ctor" (func $ctor (param f64 f64) (result i32)))
    (import "geo" "sum" (func $sum (param i32) (result f64)))
    (import "geo" "nudge" (func $nudge (param i32 f64)))
    (func (export "run") (result f64) (local $p i32)
      (local.set $p (call $ctor (f64.const 1) (f64.const 2)))
      (call $nudge (local.get $p) (f64.const 9))
      (call $sum (local.get $p)))
  )
  (core instance $mi (instantiate $m
    (with "geo" (instance
      (export "ctor" (func $ctor))
      (export "sum" (func $sum))
      (export "nudge" (func $nudge))))
  ))
  (func (export "run") (result f64) (canon lift (core func $mi "run")))
)
"#;

#[test]
fn async_methods_dispatch_and_mutate_in_place() {
    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    haphe::bind(&live_binder(), &REGISTRY, &mut linker).expect("binding succeeds");

    let result = run_guest(&engine, &linker, (), ASYNC_METHOD_GUEST).expect("guest runs");
    match result {
        Val::Float64(v) => assert!((v - 12.0).abs() < 1e-12, "got: {v}"),
        other => panic!("expected f64, got: {other:?}"),
    }
}

/// Trait projections on a resource: `default` static ctor and `eq`.
/// default() == point(0, 0) -> 1.0.
const PROJECTION_GUEST: &str = r#"
(component
  (import "haphe:demo/geometry" (instance $geo
    (export "point" (type $point (sub resource)))
    (export "[constructor]point" (func (param "x" f64) (param "y" f64) (result (own $point))))
    (export "[static]point.default" (func (result (own $point))))
    (export "[method]point.eq" (func (param "self" (borrow $point)) (param "other" (borrow $point)) (result bool)))
  ))
  (core func $ctor (canon lower (func $geo "[constructor]point")))
  (core func $default (canon lower (func $geo "[static]point.default")))
  (core func $eq (canon lower (func $geo "[method]point.eq")))
  (core module $m
    (import "geo" "ctor" (func $ctor (param f64 f64) (result i32)))
    (import "geo" "default" (func $default (result i32)))
    (import "geo" "eq" (func $eq (param i32 i32) (result i32)))
    (func (export "run") (result f64)
      (if (result f64)
        (call $eq (call $default) (call $ctor (f64.const 0) (f64.const 0)))
        (then (f64.const 1)) (else (f64.const 0))))
  )
  (core instance $mi (instantiate $m
    (with "geo" (instance
      (export "ctor" (func $ctor))
      (export "default" (func $default))
      (export "eq" (func $eq))))
  ))
  (func (export "run") (result f64) (canon lift (core func $mi "run")))
)
"#;

#[test]
fn trait_projections_dispatch_live() {
    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    haphe::bind(&live_binder(), &REGISTRY, &mut linker).expect("binding succeeds");

    let result = run_guest(&engine, &linker, (), PROJECTION_GUEST).expect("guest runs");
    match result {
        Val::Float64(v) => assert!((v - 1.0).abs() < 1e-12, "eq(default, (0,0)) held: {v}"),
        other => panic!("expected f64, got: {other:?}"),
    }
}

/// Resource params and returns through a free function: midpoint of (0,0)
/// and (4,6) has x = 2.
const MIDPOINT_GUEST: &str = r#"
(component
  (import "haphe:demo/geometry" (instance $geo
    (export "point" (type $point (sub resource)))
    (export "[constructor]point" (func (param "x" f64) (param "y" f64) (result (own $point))))
    (export "[method]point.x" (func (param "self" (borrow $point)) (result f64)))
    (export "midpoint" (func (param "a" (borrow $point)) (param "b" (borrow $point)) (result (own $point))))
  ))
  (core func $ctor (canon lower (func $geo "[constructor]point")))
  (core func $getx (canon lower (func $geo "[method]point.x")))
  (core func $mid (canon lower (func $geo "midpoint")))
  (core module $m
    (import "geo" "ctor" (func $ctor (param f64 f64) (result i32)))
    (import "geo" "getx" (func $getx (param i32) (result f64)))
    (import "geo" "mid" (func $mid (param i32 i32) (result i32)))
    (func (export "run") (result f64)
      (call $getx
        (call $mid
          (call $ctor (f64.const 0) (f64.const 0))
          (call $ctor (f64.const 4) (f64.const 6)))))
  )
  (core instance $mi (instantiate $m
    (with "geo" (instance
      (export "ctor" (func $ctor))
      (export "getx" (func $getx))
      (export "mid" (func $mid))))
  ))
  (func (export "run") (result f64) (canon lift (core func $mi "run")))
)
"#;

#[test]
fn resource_params_and_returns_cross_free_functions() {
    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    haphe::bind(&live_binder(), &REGISTRY, &mut linker).expect("binding succeeds");

    let result = run_guest(&engine, &linker, (), MIDPOINT_GUEST).expect("guest runs");
    match result {
        Val::Float64(v) => assert!((v - 2.0).abs() < 1e-12, "got: {v}"),
        other => panic!("expected f64, got: {other:?}"),
    }
}

/// Reusing an owned handle after transferring it traps (the entry was
/// consumed).
const REUSE_GUEST: &str = r#"
(component
  (import "haphe:demo/geometry" (instance $geo
    (export "point" (type $point (sub resource)))
    (export "[constructor]point" (func (param "x" f64) (param "y" f64) (result (own $point))))
    (export "[static]point.take-x" (func (param "this" (own $point)) (result f64)))
  ))
  (core func $ctor (canon lower (func $geo "[constructor]point")))
  (core func $takex (canon lower (func $geo "[static]point.take-x")))
  (core module $m
    (import "geo" "ctor" (func $ctor (param f64 f64) (result i32)))
    (import "geo" "takex" (func $takex (param i32) (result f64)))
    (func (export "run") (result f64) (local $p i32)
      (local.set $p (call $ctor (f64.const 7) (f64.const 0)))
      (drop (call $takex (local.get $p)))
      (call $takex (local.get $p)))
  )
  (core instance $mi (instantiate $m
    (with "geo" (instance
      (export "ctor" (func $ctor))
      (export "takex" (func $takex))))
  ))
  (func (export "run") (result f64) (canon lift (core func $mi "run")))
)
"#;

#[test]
fn consumed_handle_reuse_traps() {
    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    haphe::bind(&live_binder(), &REGISTRY, &mut linker).expect("binding succeeds");

    let err = run_guest(&engine, &linker, (), REUSE_GUEST).expect_err("reuse must trap");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("unknown handle") || msg.contains("consumed") || msg.contains("stale"),
        "got: {msg}"
    );
}

/// Passing a `counter` handle to a `point` member: both resources share one
/// host representation, so the cross-type misuse is caught by the binder's
/// dynamic type check (or wasmtime's handle table) and traps.
const CROSS_TYPE_GUEST: &str = r#"
(component
  (import "haphe:demo/geometry" (instance $geo
    (export "point" (type $point (sub resource)))
    (export "counter" (type $counter (sub resource)))
    (export "[method]point.x" (func (param "self" (borrow $point)) (result f64)))
    (export "[constructor]counter" (func (result (own $counter))))
  ))
  (core func $cctor (canon lower (func $geo "[constructor]counter")))
  (core func $getx (canon lower (func $geo "[method]point.x")))
  (core module $m
    (import "geo" "cctor" (func $cctor (result i32)))
    (import "geo" "getx" (func $getx (param i32) (result f64)))
    (func (export "run") (result f64)
      (call $getx (call $cctor)))
  )
  (core instance $mi (instantiate $m
    (with "geo" (instance
      (export "cctor" (func $cctor))
      (export "getx" (func $getx))))
  ))
  (func (export "run") (result f64) (canon lift (core func $mi "run")))
)
"#;

#[test]
fn cross_type_handle_misuse_traps() {
    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    haphe::bind(&live_binder(), &REGISTRY, &mut linker).expect("binding succeeds");

    let err = run_guest(&engine, &linker, (), CROSS_TYPE_GUEST).expect_err("must trap");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("belongs to") || msg.contains("resource type") || msg.contains("handle"),
        "got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Record trait projections (value semantics)
// ---------------------------------------------------------------------------

haphe::registry! {
    pub static PAIR_REGISTRY = {
        structs: [Pair],
    };
}

/// `pair-eq` compares two records by value: eq((1,2), (1,2)) -> true.
const PAIR_EQ_GUEST: &str = r#"
(component
  (import "haphe:demo/types" (instance $t
    (type $pair-def (record (field "a" f64) (field "b" f64)))
    (export "pair" (type $pair (eq $pair-def)))
    (export "pair-eq" (func (param "this" $pair) (param "other" $pair) (result bool)))
  ))
  (core module $libc (memory (export "mem") 1))
  (core instance $libc (instantiate $libc))
  (core func $eq (canon lower (func $t "pair-eq")))
  (core module $m
    (import "t" "eq" (func $eq (param f64 f64 f64 f64) (result i32)))
    (func (export "run") (result f64)
      (if (result f64)
        (call $eq (f64.const 1) (f64.const 2) (f64.const 1) (f64.const 2))
        (then (f64.const 1)) (else (f64.const 0))))
  )
  (core instance $mi (instantiate $m
    (with "t" (instance (export "eq" (func $eq))))
  ))
  (func (export "run") (result f64) (canon lift (core func $mi "run")))
)
"#;

#[test]
fn record_projection_dispatches_by_value() {
    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    let mut binder = WasmBinder::<()>::new(WitGenerator::new("haphe:demo"));
    binder.register_record::<Pair>().unwrap();
    haphe::bind(&binder, &PAIR_REGISTRY, &mut linker).expect("binding succeeds");

    let result = run_guest(&engine, &linker, (), PAIR_EQ_GUEST).expect("guest runs");
    match result {
        Val::Float64(v) => assert!((v - 1.0).abs() < 1e-12, "got: {v}"),
        other => panic!("expected f64, got: {other:?}"),
    }
}

/// Unregistered records keep descriptive projection stubs.
#[test]
fn unregistered_record_projection_traps_descriptively() {
    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    let binder = WasmBinder::<()>::new(WitGenerator::new("haphe:demo"));
    haphe::bind(&binder, &PAIR_REGISTRY, &mut linker).expect("binding succeeds");

    let err = run_guest(&engine, &linker, (), PAIR_EQ_GUEST).expect_err("stub should trap");
    let msg = format!("{err:?}");
    assert!(msg.contains("not registered"), "got: {msg}");
}

// ---------------------------------------------------------------------------
// Generic instance live dispatch (`generics` feature)
// ---------------------------------------------------------------------------

/// A generic holder resource.
#[cfg(feature = "generics")]
#[derive(Script, Clone)]
#[script(thread_safety = send_sync, methods)]
struct Holder<T: haphe::FromScript + haphe::IntoScript + Clone + Send + Sync + 'static> {
    #[script(skip)]
    v: T,
}

#[cfg(feature = "generics")]
#[haphe::script]
impl<T: haphe::FromScript + haphe::IntoScript + Clone + Send + Sync + 'static> Holder<T> {
    #[script(constructor)]
    fn new(v: T) -> Self {
        Holder { v }
    }

    fn get(&self) -> T {
        self.v.clone()
    }
}

#[cfg(feature = "generics")]
haphe::registry! {
    pub static HOLDER_REGISTRY = {
        structs: [Holder<f64>],
    };
}

#[cfg(feature = "generics")]
const HOLDER_LIVE_GUEST: &str = r#"
(component
  (import "haphe:demo/types" (instance $t
    (export "holder-f64" (type $h (sub resource)))
    (export "[constructor]holder-f64" (func (param "v" f64) (result (own $h))))
    (export "[method]holder-f64.get" (func (param "self" (borrow $h)) (result f64)))
  ))
  (core func $ctor (canon lower (func $t "[constructor]holder-f64")))
  (core func $get (canon lower (func $t "[method]holder-f64.get")))
  (core module $m
    (import "t" "ctor" (func $ctor (param f64) (result i32)))
    (import "t" "get" (func $get (param i32) (result f64)))
    (func (export "run") (result f64) (call $get (call $ctor (f64.const 2.5))))
  )
  (core instance $mi (instantiate $m
    (with "t" (instance
      (export "ctor" (func $ctor))
      (export "get" (func $get))))
  ))
  (func (export "run") (result f64) (canon lift (core func $mi "run")))
)
"#;

/// A registered monomorphized instance dispatches under its mangled name.
#[cfg(feature = "generics")]
#[test]
fn generic_instance_dispatches_live() {
    static F64_ARGS: [haphe::TypeDescriptor<'static>; 1] =
        [haphe::TypeDescriptor::Primitive(haphe::PrimitiveType::F64)];

    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    let mut binder = WasmBinder::<()>::new(WitGenerator::new("haphe:demo"));
    binder
        .register_type_instance::<Holder<f64>>(&F64_ARGS)
        .unwrap();
    haphe::bind(&binder, &HOLDER_REGISTRY, &mut linker).expect("binding succeeds");

    let result = run_guest(&engine, &linker, (), HOLDER_LIVE_GUEST).expect("guest runs");
    match result {
        Val::Float64(v) => assert!((v - 2.5).abs() < 1e-12, "got: {v}"),
        other => panic!("expected f64, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Foreign-direction guest resources
// ---------------------------------------------------------------------------

/// Guest exporting its own resource type: `make` returns an own handle,
/// `value` borrows it, `consume` takes ownership.
const STORE_GUEST: &str = r#"
(component
  (core module $m
    (import "h" "new" (func $new (param i32) (result i32)))
    (import "h" "rep" (func $rep (param i32) (result i32)))
    (import "h" "drop" (func $drop (param i32)))
    (func (export "make") (param i32) (result i32) (call $new (local.get 0)))
    ;; Borrow params lower into the defining component as raw reps; own
    ;; params re-enter its handle table (the definer regains ownership).
    (func (export "value") (param i32) (result i32) (local.get 0))
    (func (export "consume") (param i32) (result i32) (local i32)
      (local.set 1 (call $rep (local.get 0)))
      (call $drop (local.get 0))
      (local.get 1))
  )
  (type $r (resource (rep i32)))
  (core func $new (canon resource.new $r))
  (core func $rep (canon resource.rep $r))
  (core func $drop (canon resource.drop $r))
  (core instance $mi (instantiate $m
    (with "h" (instance
      (export "new" (func $new))
      (export "rep" (func $rep))
      (export "drop" (func $drop))))))
  (func $make (param "n" s32) (result (own $r)) (canon lift (core func $mi "make")))
  (func $value (param "h" (borrow $r)) (result s32) (canon lift (core func $mi "value")))
  (func $consume (param "h" (own $r)) (result s32) (canon lift (core func $mi "consume")))
  (instance $i
    (export "r" (type $r))
    (export "make" (func $make))
    (export "value" (func $value))
    (export "consume" (func $consume)))
  (export "haphe:demo/store" (instance $i))
)
"#;

/// Hand-built foreign descriptor: names drive resolution; parameter and
/// return shapes come from the guest's reflected types at dispatch time.
fn store_descriptor() -> &'static haphe::ForeignInterfaceDescriptor<'static> {
    use haphe::{
        ForeignInterfaceDescriptor, FunctionDescriptor, Ownership, Receiver, ThreadSafety,
        TypeDescriptor, TypeId,
    };
    const UNIT: TypeDescriptor<'static> = TypeDescriptor::Unit;
    const I64_PARAM: [haphe::ParamDescriptor<'static>; 1] = [haphe::ParamDescriptor {
        name: "n",
        ty: &TypeDescriptor::Primitive(haphe::PrimitiveType::I32),
        ownership: Ownership::Owned,
    }];
    const fn f(
        name: &'static str,
        params: &'static [haphe::ParamDescriptor<'static>],
    ) -> FunctionDescriptor<'static> {
        FunctionDescriptor {
            name,
            doc: None,
            receiver: Some(Receiver::Ref),
            generic_params: &[],
            instantiations: &[],
            dispatch: haphe::Dispatch::Static,
            params,
            return_type: &UNIT,
            return_ownership: Ownership::Owned,
            is_async: false,
            error_kind: None,
        }
    }
    static FNS: [FunctionDescriptor<'static>; 3] = [
        f("make", &I64_PARAM),
        f("value", &I64_PARAM),
        f("consume", &I64_PARAM),
    ];
    static DESC: ForeignInterfaceDescriptor<'static> = ForeignInterfaceDescriptor {
        id: TypeId::new("Store"),
        name: "Store",
        doc: None,
        generic_params: &[],
        functions: &FNS,
        thread_safety: ThreadSafety::NONE,
    };
    &DESC
}

#[test]
fn foreign_guest_resources_roundtrip() {
    use haphe::ScriptValue;

    let engine = Engine::default();
    let component = Component::new(&engine, wat::parse_str(STORE_GUEST).unwrap()).unwrap();
    let linker: Linker<()> = Linker::new(&engine);
    let mut store = wasmtime::Store::new(&engine, ());
    let instance = linker.instantiate(&mut store, &component).unwrap();
    let store = std::sync::Arc::new(std::sync::Mutex::new(store));
    let caller =
        haphe_wit::foreign_caller("haphe:demo", store, &instance, store_descriptor(), &[]).unwrap();

    // Own handle lifts into a GuestResource userdata.
    let h = caller
        .call("make", &[], &[ScriptValue::I64(41)])
        .expect("make succeeds");
    assert!(matches!(h, ScriptValue::UserData(_)), "got: {h:?}");

    // Borrow positions never consume: the same handle works repeatedly.
    for _ in 0..2 {
        let v = caller
            .call("value", &[], std::slice::from_ref(&h))
            .expect("value succeeds");
        assert!(matches!(v, ScriptValue::I64(41)), "got: {v:?}");
    }

    // Own transfer is single-use: the first `consume` takes the handle,
    // reuse errors descriptively.
    let v = caller
        .call("consume", &[], std::slice::from_ref(&h))
        .expect("consume succeeds");
    assert!(matches!(v, ScriptValue::I64(41)), "got: {v:?}");
    let err = caller
        .call("consume", &[], &[h])
        .expect_err("transferred handle cannot be reused");
    assert!(
        format!("{err}").contains("already-transferred"),
        "got: {err}"
    );

    // Dropped wrappers queue their handles; the next dispatch drains the
    // queue without error.
    let dropped = caller
        .call("make", &[], &[ScriptValue::I64(7)])
        .expect("make succeeds");
    drop(dropped);
    let v = caller
        .call("make", &[], &[ScriptValue::I64(1)])
        .expect("dispatch drains the drop queue");
    assert!(matches!(v, ScriptValue::UserData(_)));
}

// ---------------------------------------------------------------------------
// Value-shape coverage: tuple / flags / variant across the foreign boundary
// ---------------------------------------------------------------------------

/// Shape-heavy guest: tuple param+return, flags param+return (i32 bitmask at
/// the core level), and a variant return (flattened discriminant + payload).
#[script(foreign, thread_safety = none)]
trait Shapes {
    /// Swaps a (f64, s64) tuple.
    fn swap(&self, p: (f64, i64)) -> (i64, f64);
    /// Echoes a flags value (crosses as a list of set flag names).
    fn mark(&self, perms: Vec<String>) -> Vec<String>;
    /// Builds a variant (crosses as a single-pair map { case: payload }).
    fn make(&self, x: f64) -> std::collections::HashMap<String, f64>;
    /// Consumes a variant.
    fn measure(&self, shape: std::collections::HashMap<String, f64>) -> f64;
}

const SHAPES_GUEST: &str = r#"
(component
  (core module $m
    (memory (export "mem") 1)
    ;; Multi-value results (tuple, variant) return through a memory area at
    ;; a fixed, 8-aligned address (canonical ABI: at most one flat result).
    (func (export "swap") (param f64 i64) (result i32)
      (i64.store (i32.const 16) (local.get 1))
      (f64.store (i32.const 24) (local.get 0))
      (i32.const 16))
    (func (export "mark") (param i32) (result i32)
      (local.get 0))
    (func (export "make") (param f64) (result i32)
      (i32.store (i32.const 32) (i32.const 0))
      (f64.store (i32.const 40) (local.get 0))
      (i32.const 32))
    (func (export "measure") (param i32 f64) (result f64)
      (local.get 1)))
  (core instance $mi (instantiate $m))
  (type $perm (flags "read" "write"))
  (type $shape (variant (case "circle" f64) (case "square" f64)))
  (func $swap (param "p" (tuple f64 s64)) (result (tuple s64 f64))
    (canon lift (core func $mi "swap") (memory (core memory $mi "mem"))))
  (func $mark (param "perms" $perm) (result $perm)
    (canon lift (core func $mi "mark")))
  (func $make (param "x" f64) (result $shape)
    (canon lift (core func $mi "make") (memory (core memory $mi "mem"))))
  (func $measure (param "shape" $shape) (result f64)
    (canon lift (core func $mi "measure")))
  (instance $i
    (export "perm" (type $perm))
    (export "shape" (type $shape))
    (export "swap" (func $swap))
    (export "mark" (func $mark))
    (export "make" (func $make))
    (export "measure" (func $measure)))
  (export "haphe:demo/shapes" (instance $i))
)
"#;

#[test]
fn tuple_flags_and_variant_cross_the_foreign_boundary() {
    let engine = Engine::default();
    let component = Component::new(&engine, wat::parse_str(SHAPES_GUEST).unwrap()).unwrap();
    let linker: Linker<()> = Linker::new(&engine);
    let mut store = wasmtime::Store::new(&engine, ());
    let instance = linker.instantiate(&mut store, &component).unwrap();
    let store = std::sync::Arc::new(std::sync::Mutex::new(store));
    let shapes: ShapesHandle = haphe_wit::foreign_handle("haphe:demo", store, &instance).unwrap();

    // Tuple param and return.
    assert_eq!(shapes.swap((1.5, 7)), (7, 1.5));
    // Flags: list of set names in, same out (bitmask identity in the guest).
    assert_eq!(shapes.mark(vec!["read".into()]), vec!["read".to_string()]);
    assert_eq!(
        shapes.mark(vec!["read".into(), "write".into()]),
        vec!["read".to_string(), "write".to_string()]
    );
    // Variant return: single-pair map { case: payload }.
    let made = shapes.make(2.5);
    assert_eq!(made.get("circle"), Some(&2.5));
    assert_eq!(made.len(), 1);
    // Variant param.
    let mut square = std::collections::HashMap::new();
    square.insert("square".to_string(), 3.0);
    assert_eq!(shapes.measure(square), 3.0);
}

#[test]
fn unknown_flag_and_variant_case_error_descriptively() {
    let engine = Engine::default();
    let component = Component::new(&engine, wat::parse_str(SHAPES_GUEST).unwrap()).unwrap();
    let linker: Linker<()> = Linker::new(&engine);
    let mut store = wasmtime::Store::new(&engine, ());
    let instance = linker.instantiate(&mut store, &component).unwrap();
    let store = std::sync::Arc::new(std::sync::Mutex::new(store));
    let caller = haphe_wit::foreign_caller(
        "haphe:demo",
        store,
        &instance,
        &<ShapesHandle as haphe::ScriptForeign>::DESCRIPTOR,
        &[],
    )
    .unwrap();

    let err = caller
        .call(
            "mark",
            &[],
            &[haphe::ScriptValue::List(vec![haphe::ScriptValue::String(
                "execute".into(),
            )])],
        )
        .unwrap_err();
    assert!(
        format!("{err}").contains("a declared flag name"),
        "got: {err}"
    );

    let err = caller
        .call(
            "measure",
            &[],
            &[haphe::ScriptValue::Map(vec![(
                "triangle".into(),
                haphe::ScriptValue::F64(1.0),
            )])],
        )
        .unwrap_err();
    assert!(
        format!("{err}").contains("a declared variant case"),
        "got: {err}"
    );
}

// ---------------------------------------------------------------------------
// Host resources as foreign-call arguments
// ---------------------------------------------------------------------------

/// Guest importing the host's `point` resource, exporting a foreign
/// interface whose functions receive host-owned handles and call BACK into
/// the provided methods — proving handle identity across the boundary.
const PROBE_GUEST: &str = r#"
(component
  (import "haphe:demo/geometry" (instance $geo
    (export "point" (type $point (sub resource)))
    (export "[method]point.x" (func (param "self" (borrow $point)) (result f64)))
  ))
  (alias export $geo "point" (type $point))
  (core func $getx (canon lower (func $geo "[method]point.x")))
  (core func $dropp (canon resource.drop $point))
  (core module $m
    (import "geo" "getx" (func $getx (param i32) (result f64)))
    (import "geo" "dropp" (func $dropp (param i32)))
    (func (export "probe") (param i32) (result f64) (local $v f64)
      (local.set $v (call $getx (local.get 0)))
      ;; Borrow handles must be dropped before the call returns.
      (call $dropp (local.get 0))
      (local.get $v))
    (func (export "consume") (param i32) (result f64) (local $v f64)
      (local.set $v (call $getx (local.get 0)))
      (call $dropp (local.get 0))
      (local.get $v)))
  (core instance $mi (instantiate $m
    (with "geo" (instance
      (export "getx" (func $getx))
      (export "dropp" (func $dropp))))
  ))
  (func $probe (param "p" (borrow $point)) (result f64)
    (canon lift (core func $mi "probe")))
  (func $consume (param "p" (own $point)) (result f64)
    (canon lift (core func $mi "consume")))
  (instance $i
    (export "probe" (func $probe))
    (export "consume" (func $consume)))
  (export "haphe:demo/probe" (instance $i))
)
"#;

#[test]
fn host_resources_cross_into_foreign_calls() {
    use haphe::{
        ForeignInterfaceDescriptor, FunctionDescriptor, Ownership, PrimitiveType, Receiver,
        ThreadSafety, TypeDescriptor, TypeId,
    };

    static F64_TY: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::F64);
    static PROBE_FNS: [FunctionDescriptor<'static>; 2] = [
        FunctionDescriptor {
            name: "probe",
            doc: None,
            receiver: Some(Receiver::Ref),
            generic_params: &[],
            instantiations: &[],
            dispatch: haphe::Dispatch::Static,
            params: &[],
            return_type: &F64_TY,
            return_ownership: Ownership::Owned,
            is_async: false,
            error_kind: None,
        },
        FunctionDescriptor {
            name: "consume",
            doc: None,
            receiver: Some(Receiver::Ref),
            generic_params: &[],
            instantiations: &[],
            dispatch: haphe::Dispatch::Static,
            params: &[],
            return_type: &F64_TY,
            return_ownership: Ownership::Owned,
            is_async: false,
            error_kind: None,
        },
    ];
    static PROBE_DESC: ForeignInterfaceDescriptor<'static> = ForeignInterfaceDescriptor {
        id: TypeId::new("Probe"),
        name: "probe",
        doc: None,
        generic_params: &[],
        functions: &PROBE_FNS,
        thread_safety: ThreadSafety::NONE,
    };

    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    let binder = live_binder();
    haphe::bind(&binder, &REGISTRY, &mut linker).expect("binding succeeds");

    let component = Component::new(&engine, wat::parse_str(PROBE_GUEST).unwrap()).unwrap();
    let mut store = wasmtime::Store::new(&engine, ());
    let instance = linker.instantiate(&mut store, &component).unwrap();
    let store = std::sync::Arc::new(std::sync::Mutex::new(store));
    let caller =
        haphe_wit::foreign_caller("haphe:demo", store, &instance, &PROBE_DESC, &[]).unwrap();

    // Borrow: reusable, the guest reads through the provided method.
    let p = binder.host_resource(Point { x: 6.0, y: 0.0 }).unwrap();
    let out = caller.call("probe", &[], std::slice::from_ref(&p)).unwrap();
    assert!(matches!(out, haphe::ScriptValue::F64(v) if v == 6.0));
    let out = caller.call("probe", &[], &[p]).unwrap();
    assert!(matches!(out, haphe::ScriptValue::F64(v) if v == 6.0));

    // Own: transferred once; the guest drops it (destructor reclaims the
    // table entry); reuse errors descriptively.
    let owned = binder.host_resource(Point { x: 9.0, y: 0.0 }).unwrap();
    let out = caller
        .call("consume", &[], std::slice::from_ref(&owned))
        .unwrap();
    assert!(matches!(out, haphe::ScriptValue::F64(v) if v == 9.0));
    let err = caller.call("consume", &[], &[owned]).unwrap_err();
    assert!(
        format!("{err}").contains("already-transferred"),
        "got: {err}"
    );
}

#[test]
fn host_resource_requires_registration() {
    let binder: WasmBinder<()> = WasmBinder::new(WitGenerator::new("haphe:demo"));
    let err = binder
        .host_resource(Point { x: 0.0, y: 0.0 })
        .expect_err("unregistered type refused");
    assert!(
        format!("{err}").contains("not a registered resource type"),
        "got: {err}"
    );
}

/// Async dispatch with registry-aware resolution: the `_async_in` pair.
#[test]
fn async_handle_in_dispatches_with_registry_resolution() {
    let engine = Engine::default();
    let component = Component::new(&engine, wat::parse_str(MATH_GUEST).unwrap()).unwrap();
    let linker: Linker<()> = Linker::new(&engine);
    let mut store = wasmtime::Store::new(&engine, ());
    let instance = block_on(linker.instantiate_async(&mut store, &component)).unwrap();
    let store = std::sync::Arc::new(std::sync::Mutex::new(store));
    let validated = REGISTRY.validate().unwrap();
    let math: AsyncMathHandle =
        haphe_wit::foreign_handle_async_in("haphe:demo", store, &instance, &validated).unwrap();
    assert_eq!(block_on(math.add(20, 22)), 42);
}

// ---------------------------------------------------------------------------
// Generic-instance record projections (`generics` feature)
// ---------------------------------------------------------------------------

/// A generic data pair; `Duo<f64>` is exposed as the record `duo-f64`.
#[cfg(feature = "generics")]
#[derive(Script, Clone, Debug, PartialEq)]
#[script(thread_safety = send_sync, traits(PartialEq))]
struct Duo<T: PartialEq + haphe::FromScript + haphe::IntoScript + Clone + Send + Sync + 'static> {
    a: T,
    b: T,
}

#[cfg(feature = "generics")]
impl From<Duo<f64>> for haphe::ScriptValue {
    fn from(d: Duo<f64>) -> Self {
        haphe::ScriptValue::Map(vec![
            ("a".to_string(), haphe::ScriptValue::F64(d.a)),
            ("b".to_string(), haphe::ScriptValue::F64(d.b)),
        ])
    }
}

#[cfg(feature = "generics")]
impl haphe::FromScript for Duo<f64> {
    fn from_script(v: haphe::ScriptValue) -> Result<Self, haphe::ScriptConvertError> {
        let err = haphe::ScriptConvertError {
            expected: "Duo",
            got: v.variant_name(),
        };
        let haphe::ScriptValue::Map(pairs) = v else {
            return Err(err);
        };
        let field = |name: &str| {
            pairs
                .iter()
                .find(|(k, _)| k == name)
                .and_then(|(_, v)| match v {
                    haphe::ScriptValue::F64(n) => Some(*n),
                    _ => None,
                })
                .ok_or(err.clone())
        };
        Ok(Duo {
            a: field("a")?,
            b: field("b")?,
        })
    }
}

#[cfg(feature = "generics")]
haphe::registry! {
    pub static DUO_REGISTRY = {
        structs: [Duo<f64>],
    };
}

/// `duo-f64-eq` compares two monomorphized records: eq((1,2),(1,2)) -> true.
#[cfg(feature = "generics")]
const DUO_EQ_GUEST: &str = r#"
(component
  (import "haphe:demo/types" (instance $t
    (type $duo-def (record (field "a" f64) (field "b" f64)))
    (export "duo-f64" (type $duo (eq $duo-def)))
    (export "duo-f64-eq" (func (param "this" $duo) (param "other" $duo) (result bool)))
  ))
  (core func $eq (canon lower (func $t "duo-f64-eq")))
  (core module $m
    (import "t" "eq" (func $eq (param f64 f64 f64 f64) (result i32)))
    (func (export "run") (result f64)
      (if (result f64)
        (call $eq (f64.const 1) (f64.const 2) (f64.const 1) (f64.const 2))
        (then (f64.const 1)) (else (f64.const 0))))
  )
  (core instance $mi (instantiate $m
    (with "t" (instance (export "eq" (func $eq))))
  ))
  (func (export "run") (result f64) (canon lift (core func $mi "run")))
)
"#;

#[cfg(feature = "generics")]
#[test]
fn generic_record_instance_projection_dispatches_live() {
    static F64_ARG: [haphe::TypeDescriptor<'static>; 1] =
        [haphe::TypeDescriptor::Primitive(haphe::PrimitiveType::F64)];

    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    let mut binder = WasmBinder::<()>::new(WitGenerator::new("haphe:demo"));
    binder
        .register_record_instance::<Duo<f64>>(&F64_ARG)
        .unwrap();
    haphe::bind(&binder, &DUO_REGISTRY, &mut linker).expect("binding succeeds");

    let result = run_guest(&engine, &linker, (), DUO_EQ_GUEST).expect("guest runs");
    match result {
        Val::Float64(v) => assert!((v - 1.0).abs() < 1e-12, "got: {v}"),
        other => panic!("expected f64, got: {other:?}"),
    }
}

/// Without instance registration the projection keeps a descriptive stub.
#[cfg(feature = "generics")]
#[test]
fn unregistered_generic_record_instance_projection_traps() {
    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    let binder = WasmBinder::<()>::new(WitGenerator::new("haphe:demo"));
    haphe::bind(&binder, &DUO_REGISTRY, &mut linker).expect("binding succeeds");

    let err = run_guest(&engine, &linker, (), DUO_EQ_GUEST).expect_err("stub should trap");
    let msg = format!("{err:?}");
    assert!(msg.contains("not registered"), "got: {msg}");
}

// ---------------------------------------------------------------------------
// Async-lifted guest (component-model async, stackful)
// ---------------------------------------------------------------------------

/// A guest whose export is genuinely `async`-lifted: results return through
/// the component-model-async `task.return` intrinsic instead of the core
/// return value.
const ASYNC_LIFTED_MATH_GUEST: &str = r#"
(component
  (core func $task-return (canon task.return (result s32)))
  (core module $m
    (import "task" "return" (func $tr (param i32)))
    (func (export "add") (param i32 i32)
      (call $tr (i32.add (local.get 0) (local.get 1)))))
  (core instance $mi (instantiate $m
    (with "task" (instance (export "return" (func $task-return))))
  ))
  (type $addty (func async (param "a" s32) (param "b" s32) (result s32)))
  (func $add (type $addty)
    (canon lift (core func $mi "add") async))
  (instance $i (export "add" (func $add)))
  (export "haphe:demo/host-math" (instance $i))
)
"#;

#[test]
fn async_lifted_guest_dispatches_via_call_async() {
    let mut config = wasmtime::Config::new();
    config.wasm_component_model_async(true);
    config.wasm_component_model_async_stackful(true);
    let engine = Engine::new(&config).unwrap();
    let component =
        Component::new(&engine, wat::parse_str(ASYNC_LIFTED_MATH_GUEST).unwrap()).unwrap();
    let linker: Linker<()> = Linker::new(&engine);
    let mut store = wasmtime::Store::new(&engine, ());
    let instance = block_on(linker.instantiate_async(&mut store, &component)).unwrap();
    let store = std::sync::Arc::new(std::sync::Mutex::new(store));
    let math: AsyncMathHandle =
        haphe_wit::foreign_handle_async("haphe:demo", store, &instance).unwrap();
    assert_eq!(block_on(math.add(20, 22)), 42);
}

/// Properties and async constructors dispatch live: seeded(3) -> n=3;
/// doubled -> 6; set-doubled(10) -> n=5; set-lagged(107) -> n=7 (async
/// setter mutation persists); lagged -> 107; bump -> 8. 6+107+8 = 121.
const COUNTER_PROPS_GUEST: &str = r#"
(component
  (import "haphe:demo/geometry" (instance $geo
    (export "counter" (type $counter (sub resource)))
    (export "[static]counter.seeded" (func (param "n" s64) (result (own $counter))))
    (export "[method]counter.doubled" (func (param "self" (borrow $counter)) (result s64)))
    (export "[method]counter.set-doubled" (func (param "self" (borrow $counter)) (param "value" s64)))
    (export "[method]counter.lagged" (func (param "self" (borrow $counter)) (result s64)))
    (export "[method]counter.set-lagged" (func (param "self" (borrow $counter)) (param "value" s64)))
    (export "[method]counter.bump" (func (param "self" (borrow $counter)) (result s64)))
  ))
  (core func $seeded (canon lower (func $geo "[static]counter.seeded")))
  (core func $doubled (canon lower (func $geo "[method]counter.doubled")))
  (core func $setd (canon lower (func $geo "[method]counter.set-doubled")))
  (core func $lagged (canon lower (func $geo "[method]counter.lagged")))
  (core func $setl (canon lower (func $geo "[method]counter.set-lagged")))
  (core func $bump (canon lower (func $geo "[method]counter.bump")))
  (core module $m
    (import "geo" "seeded" (func $seeded (param i64) (result i32)))
    (import "geo" "doubled" (func $doubled (param i32) (result i64)))
    (import "geo" "setd" (func $setd (param i32 i64)))
    (import "geo" "lagged" (func $lagged (param i32) (result i64)))
    (import "geo" "setl" (func $setl (param i32 i64)))
    (import "geo" "bump" (func $bump (param i32) (result i64)))
    (func (export "run") (result i64) (local $c i32) (local $acc i64)
      (local.set $c (call $seeded (i64.const 3)))
      (local.set $acc (call $doubled (local.get $c)))
      (call $setd (local.get $c) (i64.const 10))
      (call $setl (local.get $c) (i64.const 107))
      (local.set $acc (i64.add (local.get $acc) (call $lagged (local.get $c))))
      (i64.add (local.get $acc) (call $bump (local.get $c))))
  )
  (core instance $mi (instantiate $m
    (with "geo" (instance
      (export "seeded" (func $seeded))
      (export "doubled" (func $doubled))
      (export "setd" (func $setd))
      (export "lagged" (func $lagged))
      (export "setl" (func $setl))
      (export "bump" (func $bump))))
  ))
  (func (export "run") (result s64) (canon lift (core func $mi "run")))
)
"#;

#[test]
fn properties_and_async_constructor_dispatch_live() {
    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    haphe::bind(&live_binder(), &REGISTRY, &mut linker).expect("binding succeeds");

    let result = run_guest(&engine, &linker, (), COUNTER_PROPS_GUEST).expect("guest runs");
    match result {
        Val::S64(v) => assert_eq!(v, 121),
        other => panic!("expected s64, got: {other:?}"),
    }
}

/// Without `register_type`, property accessors keep the descriptive
/// unregistered-stub message.
const COUNTER_PROP_STUB_GUEST: &str = r#"
(component
  (import "haphe:demo/geometry" (instance $geo
    (export "counter" (type $counter (sub resource)))
    (export "[constructor]counter" (func (result (own $counter))))
    (export "[method]counter.doubled" (func (param "self" (borrow $counter)) (result s64)))
  ))
  (core func $ctor (canon lower (func $geo "[constructor]counter")))
  (core module $m
    (import "geo" "ctor" (func $ctor (result i32)))
    (func (export "run") (result i64) (drop (call $ctor)) (i64.const 0))
  )
  (core instance $mi (instantiate $m
    (with "geo" (instance (export "ctor" (func $ctor))))
  ))
  (func (export "run") (result s64) (canon lift (core func $mi "run")))
)
"#;

#[test]
fn unregistered_property_stubs_keep_register_type_message() {
    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    // A binder with NOTHING registered: every member is a stub.
    let b = WasmBinder::<()>::new(WitGenerator::new("haphe:demo"));
    haphe::bind(&b, &REGISTRY, &mut linker).expect("stub binding succeeds");

    let err = run_guest(&engine, &linker, (), COUNTER_PROP_STUB_GUEST)
        .expect_err("unregistered constructor traps");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("declared but not registered; call `register_type`"),
        "got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Static generic METHOD monomorphs dispatch live (feature `generics`)
// ---------------------------------------------------------------------------

/// A box with statically dispatched generic methods.
#[cfg(feature = "generics")]
#[derive(Script, Clone)]
#[script(thread_safety = send_sync, methods)]
struct GBox {
    total: i64,
}

#[cfg(feature = "generics")]
#[script]
impl GBox {
    #[script(constructor)]
    fn new(total: i64) -> Self {
        GBox { total }
    }

    fn get(&self) -> i64 {
        self.total
    }

    /// Picks the first of two values.
    #[script(instantiate(i64), instantiate(String))]
    fn first_of<T>(&self, a: T, b: T) -> T {
        let _ = b;
        a
    }

    /// Accumulates a value in place.
    #[script(instantiate(i64))]
    fn add_in<T: Into<i64>>(&mut self, value: T) {
        self.total += value.into();
    }

    /// Fetches the total scaled by a key, asynchronously.
    #[script(instantiate(i64))]
    async fn scaled<T: Into<i64>>(&self, key: T) -> i64 {
        self.total * key.into()
    }
}

#[cfg(feature = "generics")]
haphe::registry! {
    pub static GENERIC_METHOD_REGISTRY = {
        structs: [GBox],
        modules: [
            mod boxes {
                types: [GBox],
            },
        ],
    };
}

/// first-of-s64(7, 8) = 7; add-in-s64(5) then get = 15; scaled-s64(2) = 30.
/// total = 7 + 15 + 30 = 52.
#[cfg(feature = "generics")]
const GENERIC_METHOD_GUEST: &str = r#"
(component
  (import "haphe:demo/boxes" (instance $bx
    (export "g-box" (type $gb (sub resource)))
    (export "[constructor]g-box" (func (param "total" s64) (result (own $gb))))
    (export "[method]g-box.first-of-s64" (func (param "self" (borrow $gb)) (param "a" s64) (param "b" s64) (result s64)))
    (export "[method]g-box.add-in-s64" (func (param "self" (borrow $gb)) (param "value" s64)))
    (export "[method]g-box.scaled-s64" (func (param "self" (borrow $gb)) (param "key" s64) (result s64)))
    (export "[method]g-box.get" (func (param "self" (borrow $gb)) (result s64)))
  ))
  (core func $ctor (canon lower (func $bx "[constructor]g-box")))
  (core func $first (canon lower (func $bx "[method]g-box.first-of-s64")))
  (core func $addin (canon lower (func $bx "[method]g-box.add-in-s64")))
  (core func $scaled (canon lower (func $bx "[method]g-box.scaled-s64")))
  (core func $get (canon lower (func $bx "[method]g-box.get")))
  (core module $m
    (import "bx" "ctor" (func $ctor (param i64) (result i32)))
    (import "bx" "first" (func $first (param i32 i64 i64) (result i64)))
    (import "bx" "addin" (func $addin (param i32 i64)))
    (import "bx" "scaled" (func $scaled (param i32 i64) (result i64)))
    (import "bx" "get" (func $get (param i32) (result i64)))
    (func (export "run") (result i64) (local $b i32) (local $acc i64)
      (local.set $b (call $ctor (i64.const 10)))
      (local.set $acc (call $first (local.get $b) (i64.const 7) (i64.const 8)))
      (call $addin (local.get $b) (i64.const 5))
      (local.set $acc (i64.add (local.get $acc) (call $get (local.get $b))))
      (local.set $acc (i64.add (local.get $acc) (call $scaled (local.get $b) (i64.const 2))))
      (local.get $acc))
  )
  (core instance $mi (instantiate $m
    (with "bx" (instance
      (export "ctor" (func $ctor))
      (export "first" (func $first))
      (export "addin" (func $addin))
      (export "scaled" (func $scaled))
      (export "get" (func $get))))
  ))
  (func (export "run") (result s64) (canon lift (core func $mi "run")))
)
"#;

/// Generic method monomorphs dispatch through the mangled member names:
/// `&self` picks, `&mut self` writes back in place, async drives to
/// completion on-thread.
#[cfg(feature = "generics")]
#[test]
fn generic_method_monomorphs_dispatch_live() {
    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    let mut b = WasmBinder::new(WitGenerator::new("haphe:demo"));
    b.register_type::<GBox>().unwrap();
    haphe::bind(&b, &GENERIC_METHOD_REGISTRY, &mut linker).expect("binding succeeds");

    let result = run_guest(&engine, &linker, (), GENERIC_METHOD_GUEST).expect("guest runs");
    assert!(matches!(result, Val::S64(52)), "got: {result:?}");
}

// ---------------------------------------------------------------------------
// Injected dyn dispatchers execute live (feature `dyn-generics`)
// ---------------------------------------------------------------------------

#[cfg(feature = "dyn-generics")]
trait Twice {
    fn twice(self) -> Self;
}

#[cfg(feature = "dyn-generics")]
impl Twice for i64 {
    fn twice(self) -> Self {
        self * 2
    }
}

#[cfg(feature = "dyn-generics")]
impl Twice for f64 {
    fn twice(self) -> Self {
        self * 2.0
    }
}

/// Doubles a value; `dyn` dispatch over two candidates.
#[cfg(feature = "dyn-generics")]
#[script(dyn, instantiate(i64), instantiate(f64))]
fn twice<T: Twice>(value: T) -> T {
    value.twice()
}

/// Echoes a value asynchronously; single dyn candidate.
#[cfg(feature = "dyn-generics")]
#[script(dyn, instantiate(i64))]
async fn later<T>(value: T) -> T {
    value
}

#[cfg(feature = "dyn-generics")]
haphe::registry! {
    static DYN_FN_REGISTRY = {
        modules: [
            mod dispatchers { functions: [twice, later] },
        ],
    };
}

/// Guest calling the synthesized dispatchers with each case, plus one static
/// monomorph (they remain additive). Dispatcher core shape: variant params
/// flatten to (disc: i32, payload: i64-join); the variant result spills to a
/// retptr (disc u8 at +0, payload at +8).
///
/// twice-dyn(s64(7)) = 14; twice-dyn(f64(2.5)) = 5.0 -> 5;
/// later-dyn(s64(9)) = 9; twice-s64(3) = 6. Total 34.
#[cfg(feature = "dyn-generics")]
const DYN_DISPATCHER_GUEST: &str = r#"
(component
  (type $tvt (variant (case "s64" s64) (case "f64" f64)))
  (type $lvt (variant (case "s64" s64)))
  (import "haphe:demo/dispatchers" (instance $d
    (export "twice-dyn-value" (type $tv (eq $tvt)))
    (export "twice-dyn" (func (param "value" $tv) (result $tv)))
    (export "later-dyn-value" (type $lv (eq $lvt)))
    (export "later-dyn" (func (param "value" $lv) (result $lv)))
    (export "twice-s64" (func (param "value" s64) (result s64)))
  ))
  (core module $libc (memory (export "mem") 1))
  (core instance $li (instantiate $libc))
  (core func $twice (canon lower (func $d "twice-dyn") (memory (core memory $li "mem"))))
  (core func $later (canon lower (func $d "later-dyn") (memory (core memory $li "mem"))))
  (core func $twice64 (canon lower (func $d "twice-s64")))
  (core module $m
    (import "libc" "mem" (memory 1))
    (import "d" "twice" (func $twice (param i32 i64 i32)))
    (import "d" "later" (func $later (param i32 i64 i32)))
    (import "d" "twice64" (func $twice64 (param i64) (result i64)))
    (func (export "run") (result i64) (local $acc i64)
      ;; twice-dyn(s64(7)) -> s64(14)
      (call $twice (i32.const 0) (i64.const 7) (i32.const 16))
      (local.set $acc (i64.load offset=8 (i32.const 16)))
      ;; twice-dyn(f64(2.5)) -> f64(5.0): the f64 payload crosses in the
      ;; joined i64 flat slot, reinterpreted.
      (call $twice (i32.const 1) (i64.reinterpret_f64 (f64.const 2.5)) (i32.const 32))
      (local.set $acc (i64.add (local.get $acc) (i64.trunc_f64_s (f64.load offset=8 (i32.const 32)))))
      ;; later-dyn(s64(9)) -> s64(9)
      (call $later (i32.const 0) (i64.const 9) (i32.const 48))
      (local.set $acc (i64.add (local.get $acc) (i64.load offset=8 (i32.const 48))))
      ;; static monomorph stays callable
      (local.set $acc (i64.add (local.get $acc) (call $twice64 (i64.const 3))))
      (local.get $acc))
  )
  (core instance $mi (instantiate $m
    (with "libc" (instance (export "mem" (memory $li "mem"))))
    (with "d" (instance
      (export "twice" (func $twice))
      (export "later" (func $later))
      (export "twice64" (func $twice64))))
  ))
  (func (export "run") (result s64) (canon lift (core func $mi "run")))
)
"#;

/// The dispatchers route each case to its own monomorph through the shared
/// core resolver; async candidates drive to completion on-thread; static
/// monomorphs remain callable beside the dispatcher.
#[cfg(feature = "dyn-generics")]
#[test]
fn dyn_dispatchers_execute_live() {
    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    let mut b = WasmBinder::new(WitGenerator::new("haphe:demo"));
    b.register_fn::<twice>().unwrap();
    b.register_fn::<later>().unwrap();
    haphe::bind(&b, &DYN_FN_REGISTRY, &mut linker).expect("binding succeeds");

    let result = run_guest(&engine, &linker, (), DYN_DISPATCHER_GUEST).expect("guest runs");
    assert!(matches!(result, Val::S64(34)), "got: {result:?}");
}

/// Without `register_fn`, the dispatcher stubs descriptively.
#[cfg(feature = "dyn-generics")]
#[test]
fn unregistered_dyn_dispatcher_stubs_descriptively() {
    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    let b = WasmBinder::<()>::new(WitGenerator::new("haphe:demo"));
    haphe::bind(&b, &DYN_FN_REGISTRY, &mut linker).expect("stub binding succeeds");

    let err =
        run_guest(&engine, &linker, (), DYN_DISPATCHER_GUEST).expect_err("dispatcher stub traps");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("dyn dispatcher has no registered candidates; call `register_fn`")
            || msg.contains("not registered"),
        "got: {msg}"
    );
}

/// A box with a `dyn` generic method (two candidates).
#[cfg(feature = "dyn-generics")]
#[derive(Script, Clone)]
#[script(thread_safety = send_sync, methods)]
struct DynBox {
    first: bool,
}

#[cfg(feature = "dyn-generics")]
#[script]
impl DynBox {
    #[script(constructor)]
    fn new(first: bool) -> Self {
        DynBox { first }
    }

    /// Picks one of two values.
    #[script(dyn, instantiate(i64), instantiate(f64))]
    fn pick<T>(&self, a: T, b: T) -> T {
        if self.first { a } else { b }
    }
}

#[cfg(feature = "dyn-generics")]
haphe::registry! {
    static DYN_METHOD_REGISTRY = {
        structs: [DynBox],
        modules: [
            mod dynboxes { types: [DynBox] },
        ],
    };
}

/// pick-dyn(s64(3), s64(4)) with first=true = 3; the static monomorph
/// pick-s64(5, 6) = 5 (served from the same dyn candidate table). Total 8.
#[cfg(feature = "dyn-generics")]
const DYN_METHOD_GUEST: &str = r#"
(component
  (type $pvt (variant (case "s64" s64) (case "f64" f64)))
  (import "haphe:demo/dynboxes" (instance $bx
    (export "dyn-box" (type $db (sub resource)))
    (export "dyn-box-pick-dyn-a" (type $pv (eq $pvt)))
    (export "[constructor]dyn-box" (func (param "first" bool) (result (own $db))))
    (export "[method]dyn-box.pick-dyn" (func (param "self" (borrow $db)) (param "a" $pv) (param "b" $pv) (result $pv)))
    (export "[method]dyn-box.pick-s64" (func (param "self" (borrow $db)) (param "a" s64) (param "b" s64) (result s64)))
  ))
  (core module $libc (memory (export "mem") 1))
  (core instance $li (instantiate $libc))
  (core func $ctor (canon lower (func $bx "[constructor]dyn-box")))
  (core func $pickdyn (canon lower (func $bx "[method]dyn-box.pick-dyn") (memory (core memory $li "mem"))))
  (core func $pick64 (canon lower (func $bx "[method]dyn-box.pick-s64")))
  (core module $m
    (import "libc" "mem" (memory 1))
    (import "bx" "ctor" (func $ctor (param i32) (result i32)))
    (import "bx" "pickdyn" (func $pickdyn (param i32 i32 i64 i32 i64 i32)))
    (import "bx" "pick64" (func $pick64 (param i32 i64 i64) (result i64)))
    (func (export "run") (result i64) (local $b i32) (local $acc i64)
      (local.set $b (call $ctor (i32.const 1)))
      (call $pickdyn (local.get $b) (i32.const 0) (i64.const 3) (i32.const 0) (i64.const 4) (i32.const 16))
      (local.set $acc (i64.load offset=8 (i32.const 16)))
      (local.set $acc (i64.add (local.get $acc) (call $pick64 (local.get $b) (i64.const 5) (i64.const 6))))
      (local.get $acc))
  )
  (core instance $mi (instantiate $m
    (with "libc" (instance (export "mem" (memory $li "mem"))))
    (with "bx" (instance
      (export "ctor" (func $ctor))
      (export "pickdyn" (func $pickdyn))
      (export "pick64" (func $pick64))))
  ))
  (func (export "run") (result s64) (canon lift (core func $mi "run")))
)
"#;

/// The method dispatcher unwraps both variant params (tags must agree),
/// resolves through the core scan, and calls the winning monomorph; the
/// static monomorph member serves from the same candidate table.
#[cfg(feature = "dyn-generics")]
#[test]
fn dyn_method_dispatcher_executes_live() {
    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    let mut b = WasmBinder::new(WitGenerator::new("haphe:demo"));
    b.register_type::<DynBox>().unwrap();
    haphe::bind(&b, &DYN_METHOD_REGISTRY, &mut linker).expect("binding succeeds");

    let result = run_guest(&engine, &linker, (), DYN_METHOD_GUEST).expect("guest runs");
    assert!(matches!(result, Val::S64(8)), "got: {result:?}");
}
