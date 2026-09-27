# haphe

Describe Rust types once, bind them into any embedded scripting runtime.

`haphe` provides a language-agnostic IR for Rust types, functions, and modules.
Backend crates implement `RuntimeBinder` to register types into a live
scripting runtime. Or generate binding artifacts (stubs, declarations, etc.) for
other tools to consume. The IR is const-constructible, so the compiler can verify
the registry at build time, and the runtime binding is monomorphized per backend.

Backends officially supported in this workspace:

- [`haphe-lua`](crates/haphe-lua) — mlua: live Lua registration plus LuaLS
  declaration stubs.
- [`haphe-wit`](crates/haphe-wit) — WebAssembly Component Model: `.wit`
  document generation plus live wasmtime host binding.
- [`haphe-rhai`](crates/haphe-rhai) — Rhai: live Rhai registration plus RhaiLS
  declaration stubs.

You are not limited to these backends: implement `RuntimeBinder` for any runtime
you like, and use haphe's derive macros to describe your types once and bind
them into multiple runtimes.

## Quickstart: derive the descriptors

```rust
use haphe::{Script, script};

/// A 2D point.
#[derive(Script, PartialEq)]
#[script(thread_safety = send_sync, traits(PartialEq), methods)]
struct Point {
    x: f64,
    #[script(readonly)]
    y: f64,
}

#[script]
impl Point {
    #[script(constructor)]
    fn new(x: f64, y: f64) -> Self { Point { x, y } }

    fn distance_to(&self, other: &Point) -> f64 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }
}

/// Adds two numbers.
#[script]
fn add(a: i32, b: i32) -> i32 { a + b }

haphe::registry! {
    pub static REGISTRY = {
        structs: [Point],
        modules: [
            mod math { functions: [add], types: [Point] },
        ],
    };
}
```

Everything the macros generate is a compile-time constant.Nothing is inferred:
trait impls and thread safety are declared in the attribute and **verified** —
 declaring `traits(Display)` on a type that isn't `Display`, or `thread_safety =
send_sync` on a `!Sync` type, is a compile error at the attribute (for generic
types, at each exposed instantiation). The default thread-safety claim is
`none`; only types with `async` methods must declare one explicitly (async
runtimes may or may not be multithreaded).

Descriptors can also be written by hand — the derive is a convenience layer
over the same const-constructible IR.

## Surfaces

The full declaration matrix binds — every declared surface either registers,
fails with a descriptive compile/bind error, or is descriptor-only by a
documented rule; nothing is silently dropped:

- **methods** in every receiver shape (`&self`, `&mut self`, consuming
  `self`, receiver-less associated fns), sync and `async`, plus
  constructors, computed properties (`getter`/`setter`), and free functions —
  named, or declared inline in `registry!` as annotated non-capturing
  closures (`double: |x: i64| -> i64 { x * 2 }`), which expand into
  equivalent free fns inside generated Rust modules mirroring the registry
  tree (`geometry::double`);
- **fallible surfaces** — a `Result<T, E>` return describes `T`; `Err`
  crosses as a structured error (below). `E` must implement
  `std::error::Error + Send + Sync`;
- **trait declarations** (`traits(Display, PartialEq, Add, IntoIterator, …)`)
  are verified at compile time and mapped to each runtime's native construct;
- **enums** — unit enums (string- or `#[repr]`-integer-represented), payload
  variants, `script_bitflags!` flag sets, and enum methods;
- **generics** — declared `instantiate(...)` monomorphs dispatch statically;
  `#[script(dyn, ...)]` asks the backend to pick the matching instantiation
  from the incoming values at call time (a capability backends opt into);
- **std types** — primitives, containers (`Vec`, maps, sets, tuples,
  `Option`), paths, network/time types, transparent carriers
  (`Box`/`Rc`/`Arc`/`Cow`), and transparent primitive newtypes, spelled bare
  or through explicit `std::` paths. Composites with `&str`/`&Path` elements
  cross OUTBOUND only (return types, `readonly` fields with `'static`
  references, getter-only properties) — inbound positions need owned
  element types and say so at compile time;
- **the foreign direction** — `#[script(foreign)]` traits are implemented by
  the script/guest side and called from Rust through a generated handle.

Signatures the syntactic whitelist can't judge (e.g. methods over
transparent primitive newtypes) register through
trait-presence dispatch: a real registration when the bridge traits hold, a
no-op otherwise — never a broken binding.

## Errors

A fallible implementation's error crosses the boundary **intact**:
`ScriptCallError::Callee` carries the original error as
`Arc<dyn std::error::Error + Send + Sync>` plus the declared `error_kind`
hint and the concrete type's name. Rust-side embedders `downcast_ref` to the
concrete type and walk `source()`; each backend renders data (message, kind,
type name, cause chain) into its native error construct at its own boundary.
The same applies in reverse: a structured failure from a script-side
implementation reaches the Rust caller as a `haphe::ForeignFailure` inside
`ForeignErrorKind::Call`.

## Architecture

### Embedded Runtime Model

The embedding program contains the function bodies; the scripting engine
needs types, methods, and constructors registered into its API.

```rust
// 1. Describe types (const-constructed, compile-time)
static POINT_DESC: StructDescriptor<'static> = StructDescriptor { ... };

// 2. Build and validate the registry
static REGISTRY: TypeRegistry<'static> = TypeRegistry::new(&STRUCTS, &ENUMS, &[], &[]);
let validated = REGISTRY.validate()?;

// 3. Bind into the scripting runtime
binder.bind(&validated, &mut lua_runtime)?;
```

Binding artifact files (LuaLS `---@meta` stubs, `.wit` documents) are a
secondary concern handled by `BindingGenerator`.

### Zero-Cost Design

`haphe-core` is a **build-time only** dependency. At least that's the goal.

With `'static` references, descriptors are compile-time constants. The `RuntimeBinder::bind` implementation is monomorphized per backend.

### Typestate Pipeline

The type system enforces correct usage order. You cannot bind types from an
unvalidated registry — the compiler rejects it.

```
TypeRegistry                     const-constructible, raw
    │
    ▼ .validate()
ValidatedRegistry                structural integrity proven
    │
    ├─▶ capabilities.check()     optional: verify backend compatibility
    │
    ├─▶ binder.bind(&mut rt)     primary: register into live runtime
    │
    └─▶ generator.generate()     secondary: produce binding artifact files
```

`validate()` returns `ValidatedRegistry`, not `Result<(), _>`.
`RuntimeBinder::bind()` and `BindingGenerator::generate()` accept only
`&ValidatedRegistry` — the compiler rejects unvalidated registries.

### Backend Capabilities

Each backend declares what it supports via `BackendCapabilities`:

```rust
fn capabilities(&self) -> BackendCapabilities {
    BackendCapabilities::ALL
        .with_async_fns(false)
        .with_generics(false)
        .with_required_thread_safety(Some(ThreadSafety::SEND_SYNC))
}
```

`BackendCapabilities::check()` validates a registry against these
declarations before binding, producing clear `CompatibilityError`s.
