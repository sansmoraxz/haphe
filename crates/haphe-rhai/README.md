# haphe-rhai

[Rhai](https://rhai.rs) backend for [haphe](../haphe).
Implements haphe's `RuntimeBinder` trait: describe modules, types, and
functions once with haphe's derive macros, then register them into a live
Rhai `Engine` — plus the reverse (foreign) direction, where Rust calls
functions a Rhai script supplies.

## Quickstart

```rust
use haphe::{Script, script};
use haphe_rhai::{RhaiBinder, bind_fn, bind_type};

/// A 2D point.
#[derive(Script, Clone)]
#[script(thread_safety = send_sync, methods)]
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

    fn length(&self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
}

/// Adds two numbers.
#[script]
fn add(a: i64, b: i64) -> i64 {
    a + b
}

haphe::registry! {
    pub static REGISTRY = {
        structs: [Point],
        modules: [
            mod geometry { functions: [add], types: [Point] },
        ],
    };
}

let mut engine = rhai::Engine::new();
RhaiBinder::new().bind(&REGISTRY.validate()?, &mut engine)?;

// Register types and functions:
let mut point_module = rhai::Module::new();
bind_type::<Point>(&mut engine, &mut point_module)?;
bind_fn::<add>(&mut engine)?;

// Constructors are registered under the type name (Rhai reserves `new`):
let p: Point = engine.eval("Point(3.0, 4.0)")?;

// Full field/method access:
let mut scope = rhai::Scope::new();
scope.push("p", Point::new(3.0, 4.0));
let len: f64 = engine.eval_with_scope(&mut scope, "p.length()")?;
```

Modules become static namespaces mirroring the registry hierarchy; constants
become plain Rhai values. Names cross the boundary verbatim — the exposed
(post-`rename`) haphe names, no case conversion.

## Exposing types

`#[derive(Script)]` + `#[script] impl` turn a struct into a Rhai custom type
with no hand-written Rhai registration code:

- **fields** — getters and setters (`readonly` fields get no setter);
- **constructors** — registered under the type name (`Point(x, y)`) because
  Rhai reserves `new` as a keyword;
- **methods** — `p.length()`, `p.scale(3.0)`;
- **trait operators** — `traits(Display)` → `to_string()`,
  `traits(PartialEq)` → `==`/`!=`, `traits(PartialOrd)` → `<`/`<=`/`>`/`>=`,
  arithmetic operators (`+`, `-`, `*`, `/`, `%`, `**`), bitwise operators
  (`&`, `|`, `^`, `<<`, `>>`);
- **callable objects** — `traits(Call(...))` registers an `invoke()`
  method (Rhai reserves `call` for `FnPtr`);
- **iteration** — `traits(IntoIterator(item = ...))` registers a
  `to_array()` method that materializes the iterator; iterate the result
  with Rhai's `for ... in`. `traits(Iterator)` works the same way.
  `len()` reports the iterator's size hint;
- **indexing** — `traits(Index(index = ..., output = ...))` /
  `traits(IndexMut(...))` map to `obj[key]` reads and writes;
- **computed properties** — `#[script(getter)]` / `#[script(setter)]` pairs
  bind as Rhai property accessors;
- **hash, debug** — `traits(Hash)` surfaces as `obj.hash()`,
  `traits(Debug)` as `to_debug()`.

## Free functions

`#[script]` free functions register through `bind_fn::<name>(&mut engine)`,
converting arguments and returns automatically.

## Enums

Unit enum cases are populated on the module's type sub-module, so scripts
reference them by name:

```rhai
let color = geometry::Color::Red;
```

Payload enum variants cross the boundary as a dedicated internal type
(`RhaiPayloadEnum`), distinct from regular maps — there is no ambiguity
between a map with a `"case"` key and an enum value.

## Foreign direction

A `#[script(foreign)]` trait dispatches into Rhai functions supplied as an
AST + callbacks `Map`.  The foreign caller stores a reference-counted
`Engine` and dispatches through `Engine::call_fn_with_options`, scoped to
the provided AST — only functions defined in that AST are reachable.

```rust
use std::sync::Arc;

let engine = rhai::Engine::new();
let ast = engine.compile(script)?;
let mut map = rhai::Map::new();
for f in ast.iter_functions() {
    map.insert(f.name.into(), rhai::Dynamic::from(rhai::FnPtr::new(f.name)?));
}
let math: HostMathHandle = haphe_rhai::foreign_handle(Arc::new(engine), &map, ast)?;
```

## Value conversion

| haphe value | Rhai value |
|---|---|
| unit / `Option::None` | `()` |
| bool, integers, floats | `bool`, `INT`, `FLOAT` |
| `String` | `ImmutableString` |
| `char` | `char` |
| bytes | `Blob` (`Vec<u8>`) |
| `Vec<T>` | `Array` (`Vec<Dynamic>`) |
| `HashMap<String, V>` | `Map` (`BTreeMap<SmartString, Dynamic>`) |
| user-defined types | opaque custom type |
| unit-enum values | string (case name) or integer (discriminant) |
| payload-enum values | `RhaiPayloadEnum` (internal custom type) |

## Generic functions

### Static dispatch

Statically-dispatched generic methods and associated functions are exposed
under mangled per-monomorph names (`obj.first_of__i64(4)`,
`Tally.first__string("a")`). Each declared instantiation is its own
entry — exact dispatch, no scanning. Requires the `generics` feature.

### Dyn dispatch

`dyn`-dispatched generics expose ONE function under the plain name.  At call
time, `haphe::dispatch::resolve_dyn_candidate` ranks the declared
instantiations against the incoming arguments; the ranked winner is tried
first, then remaining candidates in declaration order. Convert errors fall
through; callee errors propagate immediately.

## Callbacks

`TypeDescriptor::Callback` parameters are supported — the macro-generated
bridge handles `FromScript`/`IntoScript` conversions through Rhai's `FnPtr`.

## Declaration stubs

`RhaiDeclGenerator` implements `BindingGenerator`, emitting a
`.d.rhai` definition file describing the bound surface:

```rust
use haphe_rhai::RhaiDeclGenerator;

let output = haphe::generate(&RhaiDeclGenerator::new(), &REGISTRY)?;
// output.files[0].path == "definitions.d.rhai"
```

## Async, streams, futures

Rhai has no native async runtime.  All async method, constructor, and
function registrations are rejected at bind time with a descriptive error.
Streams and futures are unconditionally unsupported — registries carrying
them are rejected at capability check.

## Cargo features

| feature | effect |
|---|---|
| `sync` | requires `Send + Sync` for bound types (forwards to `rhai/sync`) |
| `generics` | static generic methods (mangled monomorph names), dyn candidate scanning, and generic foreign dispatch |
| `borrowed` | accepts `Cow`/borrowed signatures (compatibility shim — Rhai clones everything anyway; exists so types written for backends with real borrow semantics bind without change) |
