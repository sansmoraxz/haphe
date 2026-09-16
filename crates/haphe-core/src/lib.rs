//! Core IR and traits for the haphe scripting-language binding system.
//!
//! This crate defines a language-agnostic intermediate representation for Rust
//! types, functions, modules, and values. Backends implement [`RuntimeBinder`]
//! to register types into an embedded scripting runtime, or [`BindingGenerator`]
//! to produce binding artifact files — without ever modifying this crate.
//!
//! All IR types are lifetime-parameterized and const-constructible: with
//! `'static` references they can be defined as compile-time constants, or with
//! shorter lifetimes they can be built dynamically via [`TypeRegistryBuilder`].
//!
//! The typestate pipeline enforces correct usage at compile time:
//!
//! ```text
//! TypeRegistry → .validate() → ValidatedRegistry → binder.bind(&mut runtime)
//! ```

#[doc(hidden)]
pub mod __verify;
pub mod backend;
#[cfg(feature = "bitflags")]
mod bitflags_support;
/// Re-export of the [`bitflags`](https://docs.rs/bitflags) crate for
/// [`script_bitflags!`] expansions.
#[cfg(feature = "bitflags")]
pub use bitflags;
pub mod bridge;
pub mod foreign;
pub mod function;
pub mod haphe_type;
pub mod module;
pub mod ops;
pub mod registry;
pub mod script;
#[cfg(any(feature = "streams", feature = "futures"))]
pub mod stream;
/// Re-export of the [`futures-core`](https://docs.rs/futures-core) crate for
/// implementing custom [`stream::Stream`] sources.
#[cfg(feature = "streams")]
pub use futures_core;
pub mod types;

#[cfg(feature = "futures")]
pub use stream::Future;
#[cfg(feature = "streams")]
pub use stream::Stream;

pub use backend::{
    BackendCapabilities, BindingGenerator, CompatibilityError, GeneratedFile, GeneratedOutput,
    RuntimeBinder,
};
pub use bridge::{
    BindTarget, BridgeProbe, FnBinder, ForeignCaller, ForeignError, ForeignErrorKind,
    ForeignHandle, FromScript, IntoScript, OpaqueUserData, ScriptBind, ScriptBindFn,
    ScriptConvertError, ScriptIter, ScriptValue, SkipBind, SkipBindFn, TypeBinder,
};
pub use foreign::ForeignInterfaceDescriptor;
pub use function::{FunctionDescriptor, Ownership, ParamDescriptor, Receiver, any_async};
pub use haphe_type::HapheType;
pub use module::{ConstantDescriptor, FnInstantiation, ModuleDescriptor, union_instantiations};
pub use registry::{
    Describe, InstantiationDescriptor, RegistryError, TypeKind, TypeRegistry, TypeRegistryBuilder,
    ValidatedRegistry,
};
pub use script::{
    ScriptAlias, ScriptEnum, ScriptForeign, ScriptFunction, ScriptImpl, ScriptStruct, ScriptType,
};
pub use types::{
    EnumDescriptor, EnumVariant, FieldDescriptor, GenericParam, PrimitiveType, PropertyDescriptor,
    StructDescriptor, ThreadSafety, TraitImpl, TypeAliasDescriptor, TypeDescriptor, TypeId,
    VariantKind,
};
