//! Rhai backend for the haphe scripting-language binding generator.
//!
//! Implements [`RuntimeBinder`] to register haphe-described modules and
//! constants into a live [`rhai::Engine`], and [`bind_type`] to register
//! haphe-described types with fields, methods, constructors, and operator
//! overloads generated from `#[derive(Script)]` + `#[script] impl`.
//!
//! # What it provides
//!
//! - **Module namespaces**: static Rhai modules mirroring the module hierarchy.
//! - **Constants**: module constants converted to Rhai values.
//! - **Type binding**: [`bind_type`] registers a type's fields, methods,
//!   constructors, and trait operator overloads (Display → `to_string`,
//!   `PartialEq` → `==`/`!=`, `PartialOrd` → `<`/`<=`/`>`/`>=`, arithmetic
//!   operators, ...) as a Rhai custom type — no hand-written Rhai code required.
//! - **Foreign interfaces**: [`foreign_handle`] builds the handle for a
//!   `#[script(foreign)]` trait from a Rhai `Map` of callbacks.
//!
//! # Capabilities
//!
//! The backend declares support for properties, type aliases, foreign
//! interfaces, and callbacks. Generics (static and dyn dispatch) are
//! available behind the `generics` cargo feature. Async functions,
//! streams, and futures are not supported (Rhai has no native async
//! runtime); registries carrying them are rejected at capability check.
//! Borrowed types (`Cow`) are opt-in via the `borrowed` feature. With the `sync`
//! feature, thread safety requires `Send + Sync`; without it, no
//! thread-safety constraint is imposed.

mod binder;
mod decl;
mod error;
mod foreign;
mod module;

pub use decl::{RhaiDeclError, RhaiDeclGenerator};
pub use error::RhaiBindError;
pub use foreign::{foreign_caller, foreign_handle};

use haphe::{
    BackendCapabilities, RuntimeBinder, ScriptBind, ScriptBindFn, ScriptEnum, ScriptFunction,
    ScriptStruct, ThreadSafety, ValidatedRegistry,
};
#[cfg(not(feature = "borrowed"))]
use haphe::TypeDescriptor;

/// Rhai backend for haphe.
///
/// Registers described modules and constants into a [`rhai::Engine`].
#[derive(Debug, Clone)]
pub struct RhaiBinder {
    capabilities: BackendCapabilities,
}

impl RhaiBinder {
    pub fn new() -> Self {
        Self {
            capabilities: Self::default_capabilities(),
        }
    }

    pub fn with_capabilities(capabilities: BackendCapabilities) -> Self {
        Self { capabilities }
    }

    pub(crate) fn default_capabilities() -> BackendCapabilities {
        let thread_safety = if cfg!(feature = "sync") {
            Some(ThreadSafety::SEND_SYNC)
        } else {
            None
        };

        BackendCapabilities::ALL
            .with_async_fns(false)
            .with_callbacks(true)
            .with_streams(false)
            .with_futures(false)
            .with_generics(cfg!(feature = "generics"))
            .with_dyn_generics(cfg!(feature = "generics"))
            .with_required_thread_safety(thread_safety)
    }
}

impl Default for RhaiBinder {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeBinder for RhaiBinder {
    type Runtime = rhai::Engine;
    type Error = RhaiBindError;

    fn language_name(&self) -> &'static str {
        "rhai"
    }

    fn capabilities(&self) -> BackendCapabilities {
        self.capabilities
    }

    fn bind(
        &self,
        registry: &ValidatedRegistry<'_>,
        runtime: &mut Self::Runtime,
    ) -> Result<(), Self::Error> {
        for m in registry.modules() {
            let rhai_module = module::bind_module(registry, m)?;
            runtime.register_static_module(m.name, rhai_module.into());
        }
        Ok(())
    }
}

/// Registers a struct type's fields, methods, constructors, and trait
/// metamethods into the Rhai engine.
///
/// Call this for each type that should be usable as a custom type in Rhai.
/// Constructors and associated functions are placed on `type_module`.
pub fn bind_type<T>(
    engine: &mut rhai::Engine,
    type_module: &mut rhai::Module,
) -> Result<(), RhaiBindError>
where
    T: ScriptBind + ScriptStruct + Clone + binder::RhaiSendSync + 'static,
{
    let descriptor = <T as ScriptStruct>::DESCRIPTOR;
    #[cfg(not(feature = "borrowed"))]
    reject_borrowed_struct(&descriptor)?;
    let mut method_arities =
        build_method_arities(&[descriptor.methods, descriptor.constructors]);
    for ti in descriptor.trait_impls {
        if let haphe::TraitImpl::Call { args, .. } = ti {
            method_arities.insert("call", args.len());
        }
    }
    let mut binder = binder::RhaiTypeBinder::<T>::new();
    T::bind(&mut binder)?;
    binder.register(engine, descriptor.name, &method_arities)?;
    let _ = type_module;
    Ok(())
}

/// Binds an enum type's methods and trait metamethods into the Rhai engine.
pub fn bind_enum_type<E>(
    engine: &mut rhai::Engine,
    type_module: &mut rhai::Module,
) -> Result<(), RhaiBindError>
where
    E: ScriptBind + ScriptEnum + Clone + binder::RhaiSendSync + 'static,
{
    let descriptor = <E as ScriptEnum>::DESCRIPTOR;
    #[cfg(not(feature = "borrowed"))]
    for m in descriptor.methods {
        reject_borrowed_fn(m)?;
    }
    let method_arities = build_method_arities(&[descriptor.methods]);
    let mut binder = binder::RhaiTypeBinder::<E>::new();
    E::bind(&mut binder)?;
    binder.register(engine, descriptor.name, &method_arities)?;
    let _ = type_module;
    Ok(())
}

fn build_method_arities<'a>(
    functions: &[&'a [haphe::FunctionDescriptor<'a>]],
) -> std::collections::HashMap<&'a str, usize> {
    functions
        .iter()
        .flat_map(|fns| fns.iter())
        .map(|m| (m.name, m.params.len()))
        .collect()
}

/// Registers a free function into a Rhai module.
pub fn bind_fn<F: ScriptBindFn + ScriptFunction>(
    engine: &mut rhai::Engine,
) -> Result<(), RhaiBindError> {
    #[cfg(not(feature = "borrowed"))]
    reject_borrowed_fn(&F::DESCRIPTOR)?;
    let mut binder = binder::RhaiFnBinder::new();
    F::bind(&mut binder)?;
    binder.apply(engine, F::DESCRIPTOR.params.len());
    Ok(())
}

#[cfg(not(feature = "borrowed"))]
fn contains_borrowed(ty: &TypeDescriptor<'_>) -> bool {
    match ty {
        TypeDescriptor::Borrowed { .. } => true,
        TypeDescriptor::Option(inner)
        | TypeDescriptor::List(inner)
        | TypeDescriptor::Array(inner, _) => contains_borrowed(inner),
        TypeDescriptor::Map(key, value) | TypeDescriptor::Result(key, value) => {
            contains_borrowed(key) || contains_borrowed(value)
        }
        TypeDescriptor::Tuple(items) => items.iter().any(|t| contains_borrowed(t)),
        TypeDescriptor::Callback { params, return_type } => {
            params.iter().any(|p| contains_borrowed(p))
                || contains_borrowed(return_type)
        }
        TypeDescriptor::Instance { args, .. } => {
            args.iter().any(|a| contains_borrowed(a))
        }
        TypeDescriptor::Primitive(_)
        | TypeDescriptor::String
        | TypeDescriptor::Bytes
        | TypeDescriptor::Unit
        | TypeDescriptor::Ref(_)
        | TypeDescriptor::GenericParam(_)
        | TypeDescriptor::Stream(_)
        | TypeDescriptor::Future(_) => false,
        #[allow(
            clippy::match_same_arms,
            reason = "exhaustive listing above documents every variant; wildcard is the non_exhaustive safety net"
        )]
        _ => false,
    }
}

#[cfg(not(feature = "borrowed"))]
fn reject_borrowed_struct(
    d: &haphe::StructDescriptor<'static>,
) -> Result<(), RhaiBindError> {
    for f in d.fields {
        if contains_borrowed(f.ty) {
            return Err(RhaiBindError::UnsupportedBorrowedType {
                context: "field",
                name: f.name,
            });
        }
    }
    for m in d.methods.iter().chain(d.constructors.iter()) {
        reject_borrowed_fn(m)?;
    }
    for p in d.properties {
        if contains_borrowed(p.ty) {
            return Err(RhaiBindError::UnsupportedBorrowedType {
                context: "property",
                name: p.name,
            });
        }
    }
    Ok(())
}

#[cfg(not(feature = "borrowed"))]
fn reject_borrowed_fn(
    d: &haphe::FunctionDescriptor<'static>,
) -> Result<(), RhaiBindError> {
    for p in d.params {
        if contains_borrowed(p.ty) {
            return Err(RhaiBindError::UnsupportedBorrowedType {
                context: "function",
                name: d.name,
            });
        }
    }
    if contains_borrowed(d.return_type) {
        return Err(RhaiBindError::UnsupportedBorrowedType {
            context: "function",
            name: d.name,
        });
    }
    Ok(())
}
