# haphe

Describe Rust types once, bind them into any embedded scripting runtime.

haphe provides a language-agnostic IR for Rust types, functions, and modules.
Backend crates implement `RuntimeBinder` to register types into a live
scripting runtime (mlua, rhai, boa, steel, etc.). The binding code matches
what you'd write by hand — haphe adds nothing to your runtime binary beyond
the registration calls themselves.

## Architecture

### Embedded Runtime Model

The
host contains the function bodies; the scripting engine needs types,
methods, and constructors registered into its API.

```rust
// 1. Describe types (const-constructed, compile-time)
static POINT_DESC: StructDescriptor<'static> = StructDescriptor { ... };

// 2. Build and validate the registry
static REGISTRY: TypeRegistry<'static> = TypeRegistry::new(&STRUCTS, &ENUMS, &[], &[]);
let validated = REGISTRY.validate()?;

// 3. Bind into the scripting runtime
binder.bind(&validated, &mut lua_runtime)?;
```

Binding artifact files (`.pyi` type stubs, `.d.ts` declarations) are a
secondary concern handled by `BindingGenerator`.

### Zero-Cost Design

`haphe-core` is a **build-time only** dependency.

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
