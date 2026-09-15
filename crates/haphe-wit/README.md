# haphe-wit

WIT ([WebAssembly Component Model Interface Types](https://component-model.bytecodealliance.org/design/wit.html))
binding generator for [haphe](../haphe). Implements haphe's `BindingGenerator`
trait and produces a single `.wit` document from a type registry.

```rust
use haphe_wit::WitGenerator;

let generator = WitGenerator::new("my-ns:my-api").with_version("0.1.0");
let output = haphe::generate(&generator, &REGISTRY)?;
// output.files[0].path == "wit/host.wit"
```

## Mapping

| haphe IR | WIT |
|---|---|
| module (recursively flattened, e.g. `geometry::utils`) | `interface geometry-utils` |
| struct without methods/constructors/properties | `record` |
| struct with any of those | `resource` (fields become getter/setter funcs) |
| enum with unit variants only | `enum` |
| `#[script(flags)]` enum / `haphe::script_bitflags!` type | `flags` (bit positions follow declaration order; composite masks are compile errors) |
| enum with payload variants | `variant` (struct variants get a synthesized companion `record`) |
| enum methods | free functions `enum-name-method(this: enum-name, ...)` in the owning interface |
| type alias | `type x = y;` |
| module constants | nullary getter functions (value preserved in a doc comment); configurable via `ConstantMode` |
| `&T`/`&mut T` params where `T` is a resource | `borrow<t>` |
| `Result<T, E>` | `result<t, e>` (unit sides collapse) |
| `HashMap<K, V>` | `list<tuple<k, v>>` |
| `[T; N]` | `list<T, N>` (fixed-length list) |
| `async fn` | `async func` (component-model async, WASI 0.3+) |
| `haphe::Stream<T>` (haphe's `streams` feature) | `stream<T>` |
| `haphe::Future<T>` (haphe's `futures` feature) | `future<T>` |

Types not claimed by any module are placed in a default interface (`types`,
configurable). The generated world imports every interface — the host provides
the API and guest components consume it, mirroring haphe's embedding model.

The output targets the **WIT 0.3 feature level** (WASI 0.3 / component-model
async: `async func`, `stream<T>`/`future<T>`, fixed-length lists) with no
downlevel fallback — consumers need 0.3-aware tooling.

## Runtime binding (feature `runtime`)

The crate also implements haphe's `RuntimeBinder` for
[wasmtime](https://wasmtime.dev), behind the `runtime` cargo feature, so the
WASM target supports both halves of haphe: build-time `.wit` generation and
live host-side registration.

```rust
use haphe_wit::{WasmBinder, WitGenerator};
use wasmtime::component::Linker;

let engine = wasmtime::Engine::default();
let mut linker: Linker<()> = Linker::new(&engine);
let binder = WasmBinder::<()>::new(WitGenerator::new("my-ns:my-api"));
haphe::bind(&binder, &REGISTRY, &mut linker)?;
// guest components importing the generated world can now instantiate
```

Generator and binder take the same `WitGenerator` configuration, so linker
instance and function names always match the generated `.wit` document.

WASI is not wired implicitly: `haphe::bind` defines only the registry's own
interfaces. A guest that also imports `wasi:*` (filesystem, clocks, random,
...) needs the host to compose [`wasmtime-wasi`](https://crates.io/crates/wasmtime-wasi)
into the same linker, e.g. `wasmtime_wasi::p3::add_to_linker(&mut linker)`
(WASI 0.3) before/after `haphe::bind` — the two share the linker without
overlap.

`runtime` enables wasmtime with its default features (cranelift, async,
cache, gc, wat, ...). Non-default wasmtime extras are individually
selectable: `winch`, `pulley`, `all-arch`, `incremental-cache`, `call-hook`,
`memory-protection-keys`, `wmemcheck`, and `trace-log` (each implies
`runtime`).

Current depth: module constants are bound as real,
callable host functions; all other functions (free functions, resource
constructors/methods, enum-method companions) are stubs that trap with
"not yet implemented"; resource types get placeholder host registrations.
Full dispatch into the described Rust functions via haphe's bridge machinery
is a future tranche.

## Naming: implicit kebab-case conversion

haphe backends normally keep identifiers exactly as written in Rust. This
crate is an exception: **all identifiers are implicitly converted to
kebab-case** (`MyType` → `my-type`, `distance_to` → `distance-to`,
`MAX_VERTICES` → `max-vertices`).

This is not a stylistic choice. The WIT grammar only admits kebab-case
identifiers — there is no way to spell `MyType` or `distance_to` in a valid
WIT document, so a conversion is unavoidable. The conversion is deterministic,
and if two distinct source identifiers would collapse to the same kebab-case
name (e.g. `MyType` and `my_type`), generation fails with
`WitGenError::NameCollision` rather than silently picking one. Identifiers
that convert to WIT keywords are `%`-escaped (`Type` → `%type`) as the WIT
grammar prescribes.

Component-model bindgen tools (`wit-bindgen`, `jco`, ...) apply the inverse
conversion for each guest language, so a Rust guest sees `my-type` as `MyType`
again.

## Generics (monomorphization)

WIT has no user-defined generics, so this crate supports generic Rust types
by **monomorphization**: `registry!` records each concrete instantiation
(`structs: [Labeled<String, i32>, Labeled<bool, u8>]`), and the generator
emits one WIT type per instantiation with the generic parameters substituted.
A generic type with no recorded instantiation fails the capability check
(`UninstantiatedGeneric`), and a reference to an instantiation that was never
registered fails generation (`UnregisteredInstantiation`).

Instantiations get **deterministic mangled names**: the kebab-cased generic
name followed by one mangled fragment per argument, e.g.
`Labeled<String, i32>` → `labeled-string-s32`. Mangling is a pure function of
the type descriptors, so the same registry always produces byte-identical
output. Fragment rules:

| argument | fragment |
|---|---|
| primitive | its WIT name (`bool`, `s32`, `f64`, `char`, ...) |
| `String` / `Bytes` / `()` | `string` / `bytes` / `unit` |
| `Option<T>` / `Vec<T>` / `[T; N]` | `option-{T}` / `list-{T}` / `listN-{T}` |
| `HashMap<K, V>` | `map-{K}-{V}` |
| tuple of N | `tupleN-{T1}-...` (arity disambiguates) |
| `Result<T, E>` | `result-{T}-{E}` |
| registered type | its kebab name |
| nested instantiation | its full mangled name |

Mangled names share the type namespace, so a collision with a hand-written
type (e.g. `Labeled<bool>` vs. a struct named `LabeledBool`) is a hard
`NameCollision` error.

So that generic-backed artifacts are easy to identify, every monomorphized
type is emitted with a deterministic marker doc line before its own docs:

```wit
/// haphe:generic-instance = labeled<string, s32>
record labeled-string-s32 { ... }
```

## Validation

Every generated document is resolved through
[`wit-parser`](https://crates.io/crates/wit-parser) (the reference WIT
implementation) before being returned, so any structurally invalid shape —
recursive value types, cyclic interface `use`s, empty enums, and anything
else the WIT grammar forbids — fails `generate()` with
`WitGenError::InvalidWit` at build time instead of producing output that
breaks downstream tooling.

This wit-parser pass runs only in `generate()`. The runtime path
(`WasmBinder::bind`) shares the planning layer — package/name validation,
mangling, collision detection — but renders no document, so it never invokes
wit-parser; there, structural checking is wasmtime's job, which type-checks
every linker definition against the guest component at instantiation.

## Limitations

- `i128`/`u128`, empty tuples, and borrowed resource returns have no WIT
  equivalent and fail generation with a descriptive error.
- Callbacks are rejected by the capability check
  (`haphe::GenerateError::Incompatible`).
- WIT `constructor` cannot be `async`, so an async constructor is emitted as a
  `static async func` returning the resource instead.
- `trait_impls` metadata is not represented in the output.
