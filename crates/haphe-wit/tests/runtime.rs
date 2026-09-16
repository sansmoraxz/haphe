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
#[derive(Script, Clone)]
#[script(traits(Clone), methods)]
struct Point {
    x: f64,
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

/// Adds two integers.
#[script]
fn add(a: i32, b: i32) -> i32 {
    a + b
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
        structs: [Point],
        modules: [
            mod geometry {
                doc: "Geometry utilities",
                functions: [add, fetch_rate, counts, delayed],
                types: [Point],
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
        msg.contains("haphe-wit: `add` not yet implemented"),
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
        msg.contains("haphe-wit: `[constructor]point` not yet implemented"),
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
        msg.contains("haphe-wit: `fetch-rate` not yet implemented"),
        "got: {msg}"
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
        msg.contains("haphe-wit: `counts` not yet implemented"),
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
        msg.contains("haphe-wit: `delayed` not yet implemented"),
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
// Composite values through the foreign caller
// ---------------------------------------------------------------------------

/// A plain data pair (WIT record).
#[derive(Script, Clone, Debug, PartialEq)]
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

/// A unit enum (WIT enum); conversions are derive-generated.
#[derive(Script, Debug, PartialEq)]
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
