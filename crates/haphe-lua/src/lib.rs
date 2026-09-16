//! mlua backend for the haphe scripting-language binding generator.
//!
//! Implements [`RuntimeBinder`] to register haphe-described modules and
//! constants into a live [`mlua::Lua`] runtime, and [`bind_type`] to
//! register haphe-described types as Lua UserData — with fields, methods,
//! constructors, and metamethods generated from `#[derive(Script)]` +
//! `#[script] impl`.
//!
//! # What it provides
//!
//! - **Module tables**: nested Lua tables mirroring the module hierarchy.
//! - **Constants**: module constants converted to Lua values.
//! - **Type binding**: [`bind_type`] registers a type's fields, methods,
//!   constructors, and trait metamethods (Display → `tostring`, ToString →
//!   `..` concatenation with string-like operands, PartialEq/Eq → `==`,
//!   PartialOrd/Ord → `<`/`<=`, arithmetic operators, ...) as Lua UserData —
//!   no hand-written mlua code required.
//! - **Declaration stubs**: [`LuaDeclGenerator`] emits a LuaLS `---@meta`
//!   definition file so editors see the bound surface with full types.
//! - **Foreign interfaces**: [`foreign_handle`] builds the handle for a
//!   `#[script(foreign)]` trait from a table of Lua callbacks, so Rust calls
//!   out to script-supplied functions — no binder or registration machinery
//!   involved, just the Lua state and the table.
//!
//! # Capabilities
//!
//! The backend declares support for all IR features except async functions
//! (unless the `async` feature is enabled). With the `send` feature, thread
//! safety requires `Send` (mlua wraps userdata in `Arc<Mutex<T>>`);
//! without it, no thread-safety constraint is imposed.
//!
//! # Operator overloads
//!
//! Repeated operator-trait declarations (`traits(Add, Add(rhs = f64))`) are
//! merged into ONE Lua metamethod per operator. Resolution order:
//! `Self op Self` first, then scalar overloads whose declared rhs type
//! exactly matches the Lua value's shape (an integer prefers an integer
//! overload, a float a float one), then the remaining overloads in
//! declaration order — where a Lua integer still converts into a float
//! overload, so declaration order decides among lossy candidates.
//!
//! # Example
//!
//! ```ignore
//! use haphe::{Script, script, registry};
//! use haphe_lua::{LuaBinder, bind_type};
//!
//! #[derive(Script, Clone)]
//! #[script(thread_safety = send_sync, traits(Display), methods)]
//! struct Point { x: f64, y: f64 }
//!
//! #[script]
//! impl Point {
//!     #[script(constructor)]
//!     fn new(x: f64, y: f64) -> Self { Point { x, y } }
//! }
//!
//! registry! {
//!     pub static REGISTRY = {
//!         structs: [Point],
//!         modules: [
//!             mod math { types: [Point] },
//!         ],
//!     };
//! }
//!
//! let mut lua = mlua::Lua::new();
//! let binder = LuaBinder::new();
//! let validated = REGISTRY.validate().unwrap();
//! binder.bind(&validated, &mut lua).unwrap();
//!
//! // Register the type's UserData bindings.
//! let math: mlua::Table = lua.globals().get("math").unwrap();
//! let point_table: mlua::Table = math.get("Point").unwrap();
//! bind_type::<Point>(&lua, &point_table).unwrap();
//! ```

mod binder;
mod decl;
mod error;
mod foreign;
mod module;

pub use decl::{LuaDeclError, LuaDeclGenerator};
pub use foreign::{foreign_caller, foreign_handle};

use haphe::{
    BackendCapabilities, RuntimeBinder, ScriptBind, ScriptBindFn, ScriptStruct, ThreadSafety,
    ValidatedRegistry,
};

pub use error::LuaBindError;

/// mlua backend for haphe.
///
/// Registers described types and modules into a [`mlua::Lua`] runtime.
#[derive(Debug, Clone)]
pub struct LuaBinder {
    capabilities: BackendCapabilities,
}

impl LuaBinder {
    /// Creates a new binder with default capabilities.
    ///
    /// Defaults: all IR features supported except async (enable with the
    /// `async` crate feature). Thread safety requires `Send` only when the
    /// `send` feature is enabled; otherwise no constraint.
    pub fn new() -> Self {
        Self {
            capabilities: Self::default_capabilities(),
        }
    }

    /// Creates a binder with custom capabilities.
    pub fn with_capabilities(capabilities: BackendCapabilities) -> Self {
        Self { capabilities }
    }

    pub(crate) fn default_capabilities() -> BackendCapabilities {
        let thread_safety = if cfg!(feature = "send") {
            Some(ThreadSafety::SEND)
        } else {
            None
        };

        BackendCapabilities::ALL
            .with_async_fns(cfg!(feature = "async"))
            .with_required_thread_safety(thread_safety)
    }
}

impl Default for LuaBinder {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeBinder for LuaBinder {
    type Runtime = mlua::Lua;
    type Error = LuaBindError;

    fn language_name(&self) -> &str {
        "lua"
    }

    fn capabilities(&self) -> BackendCapabilities {
        self.capabilities
    }

    fn bind(
        &self,
        registry: &ValidatedRegistry<'_>,
        runtime: &mut Self::Runtime,
    ) -> Result<(), Self::Error> {
        let globals = runtime.globals();

        for module in registry.modules() {
            let table = module::bind_module(runtime, registry, module)?;
            globals.set(module.name, table).map_err(LuaBindError::Lua)?;
        }

        Ok(())
    }
}

/// Registers a type's fields, methods, constructors, and metamethods into
/// the Lua state. The type's `#[derive(Script)]` + `#[script] impl`
/// provides the `ScriptBind` implementation automatically.
///
/// Call this for each type that should be usable as UserData in Lua.
/// After registration, Lua code can construct, access fields, and call
/// methods on the type.
///
/// `type_table` is the Lua table where the type's constructors will be
/// placed (typically the type's entry in a module table).
///
/// # Iteration
///
/// A type declaring `traits(IntoIterator(item = ...))` (or `Iterator`)
/// iterates with the built-in `pairs(obj)` on Lua 5.2+ and LuaJIT with 5.2
/// compatibility, with Luau's native `for ... in obj do`, and with a
/// portable implicit `obj:iter()` method on every version (the only option
/// on 5.1 and plain LuaJIT, which have no `__pairs`). A declared 2-tuple
/// item iterates as `(k, v)`; any other item as 1-based `(i, item)` — this
/// pairing is decided here, statically, from the declared item type. `#obj`
/// reports the iterator's size hint. Note that built-in containers
/// (`Vec`, arrays, maps) already cross as native Lua tables and iterate
/// with plain `pairs`/`ipairs`; this covers opaque userdata types.
///
/// `ipairs(obj)` works only on Lua 5.2 and LuaJIT with 5.2 compatibility,
/// where the `__ipairs` metamethod exists: always 1-based sequential
/// `(i, item)` (the array protocol), regardless of a kv item's pairing. Lua
/// 5.3 deprecated and 5.4+ removed `__ipairs` — there `ipairs(obj)` performs
/// raw `obj[1], obj[2], ...` lookups through `__index`, which only terminates
/// on nil while this backend's `traits(Index)` bridge surfaces out-of-range
/// access as an error; the supported spellings on 5.3+ are `pairs(obj)` and
/// `obj:iter()`.
pub fn bind_type<
    T: ScriptBind + ScriptStruct + Clone + mlua::MaybeSend + mlua::MaybeSync + 'static,
>(
    lua: &mlua::Lua,
    type_table: &mlua::Table,
) -> Result<(), LuaBindError> {
    let mut binder = binder::LuaTypeBinder::<T>::new();
    binder.set_pairing(declared_iter_pairing(
        <T as ScriptStruct>::DESCRIPTOR.trait_impls,
    ));
    binder.set_idiv_fallback(declared_idiv_fallback(
        <T as ScriptStruct>::DESCRIPTOR.trait_impls,
    ));
    T::bind(&mut binder)?;
    binder.register(lua, type_table)
}

/// Binds a methods-bearing enum as full userdata: methods and trait
/// metamethods become callable on enum VALUES delivered as userdata (a
/// plain unit enum without methods needs no binding — its cases cross as
/// native string/integer values and the module case table names them).
///
/// The value boundary is unchanged: unit-enum arguments and returns still
/// convert by case name / discriminant. Userdata produced here is the
/// receiver surface for methods; a bound function returning the enum still
/// yields the lightweight value. Constructing userdata from a case is done
/// through the enum's own methods or bound functions.
pub fn bind_enum_type<
    E: ScriptBind + haphe::ScriptEnum + Clone + mlua::MaybeSend + mlua::MaybeSync + 'static,
>(
    lua: &mlua::Lua,
    type_table: &mlua::Table,
) -> Result<(), LuaBindError> {
    let mut binder = binder::LuaTypeBinder::<E>::new();
    binder.set_pairing(declared_iter_pairing(
        <E as haphe::ScriptEnum>::DESCRIPTOR.trait_impls,
    ));
    binder.set_idiv_fallback(declared_idiv_fallback(
        <E as haphe::ScriptEnum>::DESCRIPTOR.trait_impls,
    ));
    E::bind(&mut binder)?;
    binder.register(lua, type_table)
}

/// Whether `//` should fall back to the type's `Div` registration: only for
/// integer-typed `Div` declarations (integer-primitive rhs, and an output
/// that is not some other primitive shape), and never when an explicit
/// `IDiv` is declared. The fallback carries Rust's truncating semantics
/// (`-7 // 2` gives `-3`, not Lua's floor `-4`) — declare `traits(IDiv)`
/// for floor behavior on negative operands.
pub(crate) fn declared_idiv_fallback(trait_impls: &[haphe::TraitImpl<'_>]) -> bool {
    use haphe::{PrimitiveType, TraitImpl, TypeDescriptor};
    let is_int = |td: &TypeDescriptor<'_>| {
        matches!(
            td,
            TypeDescriptor::Primitive(
                PrimitiveType::I8
                    | PrimitiveType::I16
                    | PrimitiveType::I32
                    | PrimitiveType::I64
                    | PrimitiveType::U8
                    | PrimitiveType::U16
                    | PrimitiveType::U32
                    | PrimitiveType::U64
            )
        )
    };
    if trait_impls
        .iter()
        .any(|ti| matches!(ti, TraitImpl::IDiv { .. }))
    {
        return false;
    }
    trait_impls.iter().any(|ti| {
        matches!(
            ti,
            TraitImpl::Div { rhs, output }
                if is_int(rhs) && (is_int(output) || !matches!(output, TypeDescriptor::Primitive(_)))
        )
    })
}

/// The pairing for a type's iteration, from its declared iterator item type:
/// a 2-tuple item iterates as `(k, v)`, anything else as 1-based `(i, item)`.
fn declared_iter_pairing(trait_impls: &[haphe::TraitImpl<'_>]) -> binder::IterPairing {
    use haphe::{TraitImpl, TypeDescriptor};
    for ti in trait_impls {
        let item = match ti {
            TraitImpl::IntoIterator { item } | TraitImpl::Iterator { item } => item,
            _ => continue,
        };
        return if matches!(item, TypeDescriptor::Tuple(elems) if elems.len() == 2) {
            binder::IterPairing::KeyValue
        } else {
            binder::IterPairing::Enumerate
        };
    }
    binder::IterPairing::Enumerate
}

/// Registers a free function into a Lua table.
///
/// `F` is the hidden type emitted by `#[script]` on the free function.
/// The function's `ScriptBindFn` implementation provides the concrete fn
/// pointer.
///
/// ```ignore
/// #[script]
/// fn add(a: i32, b: i32) -> i32 { a + b }
///
/// let math: mlua::Table = lua.globals().get("math").unwrap();
/// bind_fn::<add>(&lua, &math).unwrap();
/// // Now lua code can call math.add(1, 2)
/// ```
pub fn bind_fn<F: ScriptBindFn>(lua: &mlua::Lua, table: &mlua::Table) -> Result<(), LuaBindError> {
    let mut binder = binder::LuaFnBinder::new();
    F::bind(&mut binder)?;
    binder.apply(lua, table)
}
