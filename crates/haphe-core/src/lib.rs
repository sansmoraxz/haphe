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

pub mod backend;
pub mod function;
pub mod module;
pub mod registry;
pub mod types;

pub use backend::{
    BackendCapabilities, BindingGenerator, CompatibilityError, GeneratedFile, GeneratedOutput,
    RuntimeBinder,
};
pub use function::{FunctionDescriptor, Ownership, ParamDescriptor, Receiver};
pub use module::{ConstantDescriptor, ModuleDescriptor};
pub use registry::{
    Describe, RegistryError, TypeKind, TypeRegistry, TypeRegistryBuilder, ValidatedRegistry,
};
pub use types::{
    EnumDescriptor, EnumVariant, FieldDescriptor, GenericParam, PrimitiveType,
    PropertyDescriptor, StructDescriptor, ThreadSafety, TraitImpl, TypeAliasDescriptor,
    TypeDescriptor, TypeId, VariantKind,
};
