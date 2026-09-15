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
//!   constructors, and trait metamethods (Display, PartialEq, Add, etc.)
//!   as Lua UserData — no hand-written mlua code required.
//!
//! # Capabilities
//!
//! The backend declares support for all IR features except async functions
//! (unless the `async` feature is enabled). With the `send` feature, thread
//! safety requires `Send` (mlua wraps userdata in `Arc<Mutex<T>>`);
//! without it, no thread-safety constraint is imposed.
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
mod error;
mod module;

use haphe::{
    BackendCapabilities, RuntimeBinder, ScriptBind, ScriptBindFn, ThreadSafety, ValidatedRegistry,
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

    fn default_capabilities() -> BackendCapabilities {
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
pub fn bind_type<T: ScriptBind + Clone + mlua::MaybeSend + mlua::MaybeSync + 'static>(
    lua: &mlua::Lua,
    type_table: &mlua::Table,
) -> Result<(), LuaBindError> {
    let mut binder = binder::LuaTypeBinder::<T>::new();
    T::bind(&mut binder)?;
    binder.register(lua, type_table)
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
