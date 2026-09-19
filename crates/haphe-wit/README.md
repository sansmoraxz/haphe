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
| struct without methods/constructors | `record` (computed properties, if any, project as interface functions: `{type}-{prop}: func(this) -> T` and, when writable, `{type}-set-{prop}: func(this, value) -> {type}` returning the UPDATED record — value semantics, like the `IndexSet` projection) |
| struct with methods or constructors | `resource` (fields and computed properties become getter/setter members) |
| enum with unit variants only | `enum` |
| `#[script(flags)]` enum / `haphe::script_bitflags!` type | `flags` (bit positions follow declaration order; composite masks are compile errors) |
| enum with payload variants | `variant` (single-field cases carry the field directly, multi-field tuple cases a `tuple<...>`, struct cases a synthesized companion `record`) — at runtime a `ScriptValue::Enum { case, payload }` lowers under the case's kebab spelling with its positional payload reassembled per that shape, and a guest variant whose case translates through the registry (and matches the declared shape) lifts back the same way; untranslated variants keep the single-pair map `{ case: payload }` convention |
| enum methods | free functions `enum-name-method(this: enum-name, ...)` in the owning interface |
| type alias | `type x = y;` |
| module constants | nullary getter functions (value preserved in a doc comment); configurable via `ConstantMode`. Primitive and string constants only: user-type constants (enum cases and payloads have no literal form — `registry!` cannot even declare them) are rejected descriptively at bind |
| `&T`/`&mut T` params where `T` is a resource | `borrow<t>` |
| `Borrowed { lifetime, inner }` (lifetime carriers, e.g. `Cow<'a, T>`) | lowered as `inner` — WIT values cross by copy, so the carried lifetime does not exist in the text |
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

Binding depth depends on registration: surfaces registered on the
`WasmBinder` (`register_type`/`register_fn`/`register_instance`/…) dispatch
LIVE into the described Rust implementations through haphe's bridge
machinery — free functions, resource constructors/methods (sync and async),
properties, generic monomorphs and `dyn` dispatchers, enum companions, trait
projections, and constants. Anything declared in the registry but not
registered is defined as a stub that traps descriptively when called, so
guests always link.

## Errors: conversion failures and fallible Rust surfaces

Every bound call wrapper yields haphe's `ScriptCallError`. Both variants map
to a **wasmtime trap** on the guest boundary:

- `Convert` (an argument or result failed to convert) traps as
  `` haphe-wit: `name`: argument/result conversion failed: … `` — unchanged
  behavior.
- `Host` (a fallible Rust implementation — a `Result<T, E>` constructor,
  method, or free function — returned `Err`) traps with the error's
  `Display` rendering, prefixed by the declared `error_kind` hint when one
  exists: `` haphe-wit: `bounded`: RangeError: negative seed -1 ``.

Fallible surfaces describe their **ok type** in WIT text (`Result<T, E>`
never appears as `result<t, e>` for these; the error has no wire
representation). Hand-built descriptors using `TypeDescriptor::Result`
explicitly are unrelated and still lower to WIT `result`. A future
refinement could lower `Host` errors into a native `result<T>` return the
way the foreign direction's `error_kind` convention does; the trap mapping
is the minimal contract today. In `dyn` dispatch, only `Convert` errors fall
through to the next candidate — a `Host` error means the matched
implementation itself failed and propagates immediately.

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

### Dispatch modes: static monomorphs emitted, `dyn` rejected

A generic function or struct/enum method declared with `instantiate(...)`
alone dispatches **statically**: this backend emits one member per declared
instantiation under the same deterministic mangled names as generic free
functions — resource methods become mangled members
(`first-of-s64: func(a: s64, b: s64) -> s64;`), enum methods become mangled
companion functions (`mode-pick-bool`), each carrying its
`haphe:generic-instance` doc marker. The `runtime` feature serves them
through the `method_generic*` binder channels, keyed on
`(name, type_args)` — sync and async alike (async monomorphs are driven to
completion on-thread like other async methods). `&mut self` generic
monomorphs on VALUE types stay unbridged like their plain siblings (no
write-back channel).

Declared `#[script(dyn, ...)]` instead asks the runtime to pick the
matching instantiation at call time by scanning the registered candidates.
WIT guests always name a monomorph statically, so without the
`dyn-generics` cargo feature this backend reports `dyn_generics: false` and
a dyn function or method fails the capability check with
`DynGenericsUnsupported`. The emitted WIT is then **dispatch-mode neutral**:
text depends only on the monomorph set, never on how a runtime dispatches
into it.

### Injected dynamic dispatch (`dyn-generics` cargo feature)

Behind the feature (which implies `generics`), dynamic dispatch is
*injected* rather than native: the capability flips to `dyn_generics: true`
and the generator additionally emits — the static monomorphs remain,
additive — one synthesized dispatcher per dyn function, per dyn method (a
resource member), and per dyn enum method (a companion function taking
`this` first). WIT admits no anonymous variants, so every generic-typed
parameter and return becomes a **named** variant type with one case per
declared instantiation, the case named by the instantiation's mangled type
arguments:

```wit
/// Candidate payloads for the `echo` dynamic dispatcher.
variant echo-dyn-value {
  %s64(s64),
  %string(string),
}
/// haphe:dyn-dispatcher = echo
echo-dyn: func(value: echo-dyn-value) -> echo-dyn-value;
```

Variant names derive from the dispatcher and position
(`{fn}-dyn-{param}` / `{fn}-dyn-result`, owner-prefixed for methods, e.g.
`holder-echo-dyn-value`); identical shapes deduplicate onto the first
definition within an interface, so `echo`'s parameter and return share one
type above. Method dispatcher variants are emitted at interface level,
before the resource that uses them. Non-generic positions pass through
unchanged; a name collision with a hand-written type surfaces through the
wit-parser validation pass.

The `runtime` host implements each dispatcher by reading the case tag
(which names the instantiation, so matching degenerates to exact), running
the SAME shared core resolver (`haphe::dispatch::resolve_dyn_candidate`)
over the candidates collected through the `function_dyn*`/`method_dyn*`
binder channels, calling the winner (rejected conversions fall through in
declaration order), and lifting the result back under the winning case.
Multiple variant-typed parameters must carry the SAME tag — they all name
the one instantiation being selected — and a tag matching no candidate
traps descriptively, listing every declared candidate. The static monomorph
members of a dyn function or method serve from the same candidate table
(dyn registrations flow only through the dyn channels).

One documented divergence: a dyn METHOD dispatcher acquires its receiver
by borrow-and-clone per attempt — a consuming (`self`) candidate does not
consume the handle, keeping fall-through possible.

**Generic self types.** A dyn method on a generic self type composes two
substitutions. In the TEXT, positions typed by the SELF type's parameters
render concretely per resource monomorph (they are fixed there — only the
method's own parameters become dispatcher cases); statically dispatched
generic methods likewise compose the resource instantiation's environment
with the method's own (`mix-f64: func(base: s64, k: f64) -> f64` on
`dial-s64`). At RUNTIME each candidate carries its
`haphe::SelfInstantiation`; ranking merges the self parameters/arguments
with the candidate's own before running the same core matcher
(`value_matches_descriptor`), so a self-typed parameter ranks against the
monomorph's concrete type.

### Foreign dispatch modes

Generic FOREIGN methods declare their dispatch mode at the source
(`instantiate(...)` alone = static, `dyn` = erased); the Rust call site is
always concretely typed, so foreign dispatch is pure ADDRESSING — never
resolution.

- **Static**: the guest exports one function per declared instantiation
  under the mangled member name (`parse-s64`), same scheme as provided
  monomorphs. A call whose type arguments match no declared instantiation
  fails descriptively, naming the declared set.
- **`dyn`** (feature `dyn-generics`): the guest exports exactly ONE function
  under the PLAIN name — no `-dyn` suffix, since erased addressing replaces
  the monomorphs rather than standing beside them — whose generic-typed
  positions are the same synthesized case variants (marker doc line
  `haphe:dyn-foreign = {fn}`). The caller wraps each call's concrete type
  arguments into the matching case (a tag lookup, no resolver), so the case
  tag still tells the guest which instantiation was meant, and unwraps the
  return expecting the same case back — an undeclared instantiation or a
  mismatched return case fails descriptively. Without the feature, `dyn`
  foreign functions are rejected by the capability check (and, on the
  direct caller API, by `WitGenError::DynForeignFunction`).

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
- Receiver-less associated functions emit as `static func` members but their
  runtime binding traps descriptively ("not yet bridged"): the typed bridge
  erasure cannot fabricate a receiver, so bridging them needs a
  receiver-less `TypeBinder` channel in haphe-core (tracked upstream).
- Declared `trait_impls` are PROJECTED into WIT-native named functions
  (interusability: every trait a type declares is reachable from guests).
  On resources they are members; on records they are interface-level
  functions named `{type}-{member}` taking `this` by value (mutating
  projections return the updated record). The deterministic table:

  | Trait | WIT member |
  |---|---|
  | `Add`/`Sub`/`Mul`/`Div`/`Rem`/`IDiv`/`Mod`/`Pow`/`BitAnd`/`BitOr`/`BitXor`/`Shl`/`Shr` | `add`/`sub`/… `: func(rhs: borrow<T> \| SCALAR) -> OUT` |
  | `Neg` / `Not` | `neg`/`not: func() -> OUT` |
  | `PartialEq`/`Eq` (deduplicated) | `eq: func(other: borrow<T>) -> bool` |
  | `PartialOrd`/`Ord` (deduplicated) | `lt`/`le: func(other: borrow<T>) -> bool` |
  | `Display`/`ToString` (deduplicated) | `to-string: func() -> string` |
  | `Debug` | `to-debug-string: func() -> string` |
  | `Hash` | `hash: func() -> u64` |
  | `Call` / `AsyncCall` | `call: [async] func(a0: A, …) -> O` (declaring both is an error) |
  | `Index` / `IndexMut` | `at: func(index: I) -> OUT` / `set-at: func(index: I, value: OUT)` |
  | `Iterator`/`IntoIterator` (deduplicated) | `items: func() -> list<ITEM>` + `length: func() -> u64` — an **eager snapshot** (lazy iteration is not WIT-native; the semantic difference is documented in the generated doc comment) |
  | `Default` | `default: static func() -> T` (never the `constructor` slot) |
  | `Clone` | unprojected (handles give guests sharing; value types copy structurally) |

  Projected names share the member namespace: a user member spelled `add`,
  `at`, `call`, … collides with the projection as a descriptive generation
  error, as do multiple overloads of one operator — expose a named method
  instead. Enum `trait_impls` are not yet projected.
