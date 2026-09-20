# haphe-lua

[mlua](https://crates.io/crates/mlua) backend for [haphe](../haphe).
Implements haphe's `RuntimeBinder` trait: describe modules, types, and
functions once with haphe's derive macros, then register them into a live
Lua state — plus the reverse (foreign) direction, where Rust calls functions
a Lua script supplies.

## Quickstart

```rust
use haphe::{Script, script};
use haphe_lua::{LuaBinder, bind_fn, bind_type};

/// A 2D point.
#[derive(Script)]
#[script(methods)]
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

let mut lua = mlua::Lua::new();
LuaBinder::new().bind(&REGISTRY.validate()?, &mut lua)?;

// Live dispatch (replaces the module stubs):
let geometry: mlua::Table = lua.globals().get("geometry")?;
let point_table: mlua::Table = geometry.get("Point")?;
bind_type::<Point>(&lua, &point_table)?;
bind_fn::<add>(&lua, &geometry)?;

// Lua sees plain tables and userdata:
lua.load("local p = geometry.Point.new(3, 4); return geometry.add(1, 2)")
    .eval::<i64>()?;
```

Modules become nested global tables mirroring the registry hierarchy;
constants become plain Lua values on their module table.

## Exposing types

`#[derive(Script)]` + `#[script] impl` turn a struct into Lua UserData with
no hand-written mlua code:

- **fields** — getters and setters (`readonly` fields get no setter);
- **methods and constructors** — `p:length()`, `Point.new(x, y)`;
- **trait metamethods** — `traits(Display)` → `tostring(p)`,
  `traits(ToString)` → `..` concatenation (operands must be string-like:
  this type, strings, or numbers — anything else errors),
  `traits(PartialEq)` → `==`, comparison and arithmetic operators map to
  their Lua metamethods;
- **iteration** — `traits(IntoIterator(item = ...))` (or `Iterator`) makes
  the userdata iterable: `pairs(obj)` on Lua 5.2+ and LuaJIT with 5.2
  compatibility, native `for ... in obj do` on Luau, and a portable implicit
  `obj:iter()` method on every version (the only option on 5.1 and plain
  LuaJIT, which lack `__pairs`). A declared 2-tuple item iterates as
  `(k, v)`; any other item as 1-based `(i, item)`. Each iteration clones the
  value and holds its own lazy iterator, so concurrent iterations are
  independent snapshots. `#obj` reports the iterator's size hint (exact for
  standard containers). `ipairs(obj)` works only on 5.2/LuaJIT-5.2-compat
  (the `__ipairs` metamethod era) and is always 1-based sequential; 5.3
  deprecated and 5.4+ removed the metamethod, so there — and on 5.1 —
  `pairs(obj)` (where available) and `obj:iter()` are the supported
  spellings. Built-in containers (`Vec`, arrays, maps) already
  cross as native Lua tables — this is for opaque userdata types;
- **pow** — `traits(Pow)` (the haphe-provided `haphe::ops::Pow` trait; the
  standard library has none) maps to `x ^ y` on every supported Lua version;
- **bitwise operators** — `traits(BitAnd, BitOr, BitXor, Shl, Shr, Not)` map
  to `&`, `|`, `~` (binary), `<<`, `>>`, `~` (unary) on **Lua 5.3+ only**:
  Lua 5.1/5.2, LuaJIT, and Luau have no bitwise metamethods, and binding a
  bitwise-declared type there fails with a descriptive error rather than
  silently dropping the operator;
- **modulo** — `%` dispatches to `traits(Mod)` (haphe's `ops::Mod`, Lua's
  own floor semantics: the result takes the divisor's sign) when declared,
  else to `traits(Rem)` with Rust's truncated semantics (dividend's sign —
  differs on negative operands). `Mod` wins when both are declared; declare
  it for genuine Lua `%` behavior. Works on every Lua version;
- **floor division** — `traits(IDiv)` (haphe's `ops::IDiv`, floor
  semantics: the quotient rounds toward negative infinity, matching Lua's
  `//` exactly) maps to `//` on Lua 5.3+ and Luau; earlier versions reject
  the binding. A type with no `IDiv` but an integer-typed `Div` declaration
  also answers `//` through its `Div` — with Rust's TRUNCATING semantics
  (`-7 // 2` gives `-3` there, not Lua's `-4`): declare `IDiv` for floor
  behavior on negative operands;
- **callable types** — `traits(Call(args = (A, B), output = O))` (haphe's
  `ops::Call`; args are always a tuple) maps to `__call`, so `obj(a, b)`
  works on every Lua version. `traits(AsyncCall(...))` (haphe's
  `ops::AsyncCall`, requires an explicit `thread_safety` declaration) binds
  an async `__call` driven through mlua's async support — it needs this
  backend's `async` feature, is unavailable on Lua 5.1/Luau and under the
  `send` feature (async call futures are not `Send`), and each unsupported
  combination is a descriptive registration error. Declaring both `Call`
  and `AsyncCall` on one type is rejected as ambiguous (Lua has a single
  `__call`);
- **async methods** — `async fn` in a `#[script] impl` (explicit
  `thread_safety` required) binds through mlua's async methods on every Lua
  version, behind this backend's `async` feature (and not under `send`:
  async futures are not `Send`); unsupported combinations are descriptive
  registration errors. Dispatch is zero-clone: borrowed receivers pass
  through mlua's guard held across `await`s, and `&mut self` async methods
  hold the mutable guard so mutation writes back to the bound userdata in
  place;
- **async free functions** — a `#[script] async fn` registered via
  `bind_fn` binds through mlua's async functions under the same gating as
  async methods (`async` feature, not `send`); unsupported combinations are
  descriptive registration errors naming the function;
- **computed properties** — `#[script(getter)]` / `#[script(setter)]` pairs
  bind as Lua field accessors: `obj.level` reads through the getter,
  `obj.level = v` writes through the setter (a getter without a setter is
  read-only). A property sharing a name with a plain field is a descriptive
  registration error. Async accessors are declared in core but mlua exposes
  no async field accessors, so they are rejected descriptively — declare an
  async method for awaited access;
- **async constructors** — `#[script(constructor)] async fn` lands on the
  type table like a sync constructor and is awaited when called
  (`Type.connect(...)`), under the same gating as async functions; sync and
  async constructors share one namespace (duplicates are rejected);
- **associated functions** — a receiver-less fn in a `#[script] impl` lands
  on the type table next to constructors and is called with no instance:
  `Meter.flipped(false)`. Sync, async (same gating as async methods), and
  fallible forms all work; static generic monomorphs live under mangled
  names (`Tally.first__i64(...)`) and `dyn` generics under the bare name
  with runtime candidate scanning. Constructors, associated functions, and
  generic mangles share the type-table namespace — duplicates are rejected
  loudly;
- **indexing** — `traits(Index(index = ..., output = ...))` /
  `traits(IndexMut(...))` map to `obj[key]` reads and writes. Registered
  fields and methods win; the custom index is the fallback for misses. Keys
  convert verbatim to the declared Rust index type (no 0/1-based
  adjustment), and a Rust out-of-bounds panic surfaces as a Lua error.
- **hash, debug, default** — Lua has no hashing protocol, so `traits(Hash)`
  surfaces as a portable `obj:hash()` method (the Rust `Hash` digest as a
  Lua integer). `traits(Debug)` fills mlua's `__todebugstring` metamethod
  (an mlua extension consulted before `__tostring` when Rust pretty-formats
  the userdata) plus a portable `obj:debug()` companion for scripts; and
  `tostring(obj)` still falls back to the `{:?}` formatting when no
  `Display` is declared. `traits(Default)` registers an implicit
  nullary constructor, `Type.default()`. User members named `hash`/`debug`
  on declaring types (or a second constructor named `default`) are
  registration errors, never silent shadows — the same trait declarations
  stay reachable from every haphe backend.

Operator declarations bind against haphe's operator traits
(`haphe::ops::Add`, `haphe::ops::Pow`, ...). Each is a blanket extension of
its `core::ops` counterpart, so a type implementing the standard trait
satisfies the declaration automatically; `Pow` (which std lacks) is
implemented directly, or via `num_traits::Pow` with haphe's `num-traits`
feature.

Repeated operator declarations (`traits(Add, Add(rhs = f64), Add(rhs =
i64))`) merge into one metamethod per operator, resolved as:

1. `Self op Self` (both operands are the userdata type);
2. scalar overloads whose declared rhs type exactly matches the Lua value's
   shape — an integer prefers an integer overload, a float a float one;
3. the remaining overloads in declaration order — a Lua integer still
   converts into a float overload here, so declaration order decides among
   lossy candidates.

## Free functions

`#[script]` free functions listed in a module register through
`bind_fn::<name>(&lua, &module_table)`, converting arguments and returns
automatically. Until `bind_fn` runs, the module table holds a stub that
raises a descriptive error.

### Generic functions: static vs `dyn` dispatch

Statically-dispatched generic FREE functions (`#[script(instantiate(...))]`
without `dyn`) are rejected by this backend — module tables dispatch by name
only, so named monomorphs would shadow each other. Generic METHODS and
associated functions dispatch statically under mangled per-monomorph names
instead (`p:first_of__i64(4, 9)`, `Tally.first__string('a', 'b')`) — each
declared instantiation is its own entry, exact dispatch, no scan.

Declaring `dyn` makes the generic callable by its bare name:

```rust
#[script(dyn, instantiate(i64), instantiate(String))]
fn echo<T>(value: T) -> T { value }
```

A bare `#[script(dyn)]` on a single-parameter generic auto-instantiates the
default candidate set `{i64, f64, bool, String, char}` — the order is also
the dispatch priority (a single-character Lua string therefore lands on
`String`, which precedes `char`). Explicit `instantiate(...)` declarations
fully override the default set; multi-parameter generics always require
them.

```lua
m.echo(5)     -- dispatches to the i64 instantiation
m.echo("hi")  -- dispatches to the String instantiation
```

One Lua function scans the declared instantiations at call time: a first
pass takes a candidate whose every parameter matches the arguments exactly,
a second accepts coercible matches (e.g. an integer for a float parameter),
and ties resolve in declaration order. The chosen candidate's conversions
stay authoritative — if they reject the values the remaining candidates are
try-called in declaration order, and when nothing accepts, the error lists
every candidate signature. Async `dyn` functions dispatch the same way, and
so do `dyn` METHODS and associated functions — one entry under the bare name
(`obj:mirror(5)`, `obj:mirror("hi")`), scanning that member's candidates; on
a generic self type the resource monomorph's own type arguments join the
ranking.

`dyn` dispatch rides the `generics` cargo feature (the backend reports the
`dyn_generics` capability only when it is enabled); without it, `dyn`
registrations are rejected descriptively.

## Errors

A fallible surface (`Result<T, E>` constructor, method, associated fn, or
free function; `E: std::error::Error + Send + Sync`) raises a Lua error on
`Err`. `tostring(e)` renders `"Kind: message"` when the declaration carries
`#[script(error_kind = "Kind")]`, the bare message otherwise — but the error
VALUE is the intact `ScriptCallError` crossing as an external error, and the
`haphe_error` global (installed by `bind()`; `install_error_info(&lua)` for
à-la-carte `bind_type`/`bind_fn` composition) decodes one caught by `pcall`
into structured fields:

```lua
local ok, e = pcall(function() return m:checked_add(big) end)
local info = haphe_error(e)
-- info.message  the error's Display rendering
-- info.kind     the declared error_kind, or nil
-- info.type     the Rust error type's name
-- info.chain    rendered source() causes, outermost first
```

`haphe_error` returns `nil` for anything that isn't a callee error (plain
string errors, argument conversion failures). Conversion failures stay plain
string errors — boundary misuse, not a structured outcome.

## Value conversion

Names cross the boundary verbatim — the exposed (post-`rename`) haphe names,
no case conversion.

| haphe value | Lua value |
|---|---|
| unit / `Option::None` | `nil` |
| bool, integers, floats | `boolean`, `integer`, `number` |
| `String`, bytes, `char` | `string` |
| `Vec<T>` | sequence table |
| `HashMap<String, V>` | string-keyed table |
| user-defined types | userdata (opaque) |
| unit-enum values | string enums: the declared case name as a `string`, matched exactly on the way back (`"DarkBlue"`, never `"dark-blue"`); numeric enums (a Rust `#[repr]` integer type): the discriminant as an `integer` |
| transparent primitive newtypes | the native primitive |

Transparent newtypes over primitives cross as the native primitive in both
directions — a `#[script(transparent)] struct Toggle(bool)` return value is a
real Lua `boolean` (so `not v` and `if v then` behave; userdata can never
participate in truthiness, since only `nil` and `false` are falsy), and a
parameter of that type accepts a plain boolean. Opaque newtypes remain
distinct declaration-level types with no generated value conversions.

## Enums

A unit enum's representation follows its Rust declaration: without a
`#[repr]` it crosses as its declared case names (strings); with a Rust
integer `#[repr]` the exact type propagates and values cross as integers —
explicit discriminants (`Low = 1`) and Rust's implicit chain are respected,
and a plain integer argument converts when it matches a declared
discriminant. `script_bitflags!` enums carry their real bit values.

Every enum's module table is populated with its cases, so scripts reference
them by name instead of magic values:

```lua
palette.Color.Red    -- "Red"      (string enum)
palette.Level.Low    -- 1          (#[repr(u8)] enum)
if lvl == palette.Level.High then ... end
```

A `#[script(methods)]` enum additionally binds as full userdata via
`bind_enum_type::<E>()` (the enum analogue of `bind_type`): methods and
trait metamethods become callable on userdata receivers, while plain enum
values keep the lightweight string/integer representation at the function
boundary.

## Foreign direction

A `#[script(foreign)]` trait dispatches into Lua functions supplied as a
callbacks table. Building the handle involves no binder or registration
machinery — it only connects to an existing Lua state and table. Every
function is resolved when the handle is built; a missing or non-function
field is an immediate `LuaBindError::MissingForeignFunction`, not a
call-time surprise.

```rust
let callbacks: mlua::Table = lua
    .load(r#"return { add = function(a, b) return a + b end }"#)
    .eval()?;
let math: HostMathHandle = haphe_lua::foreign_handle(&lua, callbacks)?;
assert_eq!(math.add(2, 3), 5);
```

A Lua `error(...)` inside a callback surfaces through `Result`-returning
trait methods (`E: From<ForeignError>`); non-`Result` methods panic on
failure.

## Declaration stubs

`LuaDeclGenerator` implements haphe's `BindingGenerator`, emitting a LuaLS
(`lua-language-server`) `---@meta` definition file describing the same
surface the runtime binder exposes — classes with fields, methods, and
`---@operator` annotations, module tables with typed functions and
constants, unit enums as `---@enum` tables mirroring the runtime case
tables (string names or numeric discriminants per repr), and foreign
interfaces as the callbacks-table shape a script must supply.

```rust
use haphe_lua::LuaDeclGenerator;

let output = haphe::generate(&LuaDeclGenerator::new(), &REGISTRY)?;
// output.files[0].path == "definitions.lua"
```

## Cargo features

| feature | effect |
|---|---|
| `lua55` (default) / other runtime selectors | selects the Lua version mlua links against |
| `async` | advertises async support and dispatches async foreign methods via `Function::call_async` |
| `send` | requires `Send` thread safety for bound types; makes the Lua handles `Send` |
| `error-send` | `Send + Sync` mlua errors |
| `serialize` | mlua serde integration |
| `generics` | generic dispatch, both directions: export-side static generic methods/associated fns (mangled monomorph names) and `dyn` candidate scanning (`dyn_generics` capability); foreign-side generic interfaces/functions (Lua is dynamic, so one Lua function serves every instantiation — type arguments are ignored). Without it, generic use is a descriptive error. Static generic FREE functions remain unsupported in the export direction (module-table monomorphs would collide by name — declare `dyn`). |
