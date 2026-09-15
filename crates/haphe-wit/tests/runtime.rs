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

haphe::registry! {
    pub static REGISTRY = {
        structs: [Point],
        modules: [
            mod geometry {
                doc: "Geometry utilities",
                functions: [add, fetch_rate],
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
fn run_guest(
    engine: &Engine,
    linker: &Linker<()>,
    guest_wat: &str,
) -> Result<Val, wasmtime::Error> {
    let component = Component::new(engine, wat::parse_str(guest_wat)?)?;
    let mut store = wasmtime::Store::new(engine, ());
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

    let result = run_guest(&engine, &linker, PI_GUEST).expect("guest runs");
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

    let err = run_guest(&engine, &empty_linker, PI_GUEST).expect_err("missing import");
    let msg = format!("{err:?}");
    assert!(msg.contains("haphe:demo/geometry"), "got: {msg}");
}

/// Functions are bound at Lua-parity depth: the definition exists (the guest
/// links and instantiates) but calling it traps with a clear message.
#[test]
fn stub_function_links_but_traps_when_called() {
    let engine = Engine::default();
    let mut linker = Linker::new(&engine);
    haphe::bind(&binder(), &REGISTRY, &mut linker).expect("binding succeeds");

    let err = run_guest(&engine, &linker, ADD_GUEST).expect_err("stub should trap");
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

    let err = run_guest(&engine, &linker, POINT_GUEST).expect_err("ctor stub should trap");
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

    let err = run_guest(&engine, &linker, ASYNC_GUEST).expect_err("stub should trap");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("haphe-wit: `fetch-rate` not yet implemented"),
        "got: {msg}"
    );
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
    static REGISTRY_BAD: haphe::TypeRegistry = haphe::TypeRegistry::new(&STRUCTS, &[], &[], &[]);

    let engine = Engine::default();
    let mut linker = Linker::<()>::new(&engine);
    match haphe::bind(&binder(), &REGISTRY_BAD, &mut linker) {
        Err(haphe::BindError::Bind(WasmBindError::Gen(
            haphe_wit::WitGenError::NameCollision { .. },
        ))) => {}
        other => panic!("expected NameCollision, got: {other:?}"),
    }
}
