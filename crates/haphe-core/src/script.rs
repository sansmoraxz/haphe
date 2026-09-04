use crate::function::FunctionDescriptor;
use crate::types::{
    EnumDescriptor, PropertyDescriptor, StructDescriptor, TypeAliasDescriptor, TypeId,
};

/// A type with a unique identity in a [`TypeRegistry`](crate::TypeRegistry).
///
/// Implemented by `#[derive(Script)]`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a scriptable type",
    note = "add `#[derive(Script)]` to `{Self}`"
)]
pub trait ScriptType {
    /// The unique registry identifier of this type.
    const ID: TypeId<'static>;
}

/// A struct with a compile-time [`StructDescriptor`].
///
/// Implemented by `#[derive(Script)]` on a struct.
#[diagnostic::on_unimplemented(
    message = "`{Self}` has no struct descriptor",
    note = "structs listed in `registry!` under `structs:` need `#[derive(Script)]`",
    note = "if `{Self}` is an enum, list it under `enums:` instead"
)]
pub trait ScriptStruct: ScriptType {
    /// The struct's descriptor.
    const DESCRIPTOR: StructDescriptor<'static>;
}

/// An enum with a compile-time [`EnumDescriptor`].
///
/// Implemented by `#[derive(Script)]` on an enum.
#[diagnostic::on_unimplemented(
    message = "`{Self}` has no enum descriptor",
    note = "enums listed in `registry!` under `enums:` need `#[derive(Script)]`",
    note = "if `{Self}` is a struct, list it under `structs:` instead"
)]
pub trait ScriptEnum: ScriptType {
    /// The enum's descriptor.
    const DESCRIPTOR: EnumDescriptor<'static>;
}

/// A newtype exposed as a type alias, implemented by `#[derive(Script)]` on a
/// single-field tuple struct.
#[diagnostic::on_unimplemented(
    message = "`{Self}` has no type-alias descriptor",
    note = "types listed in `registry!` under `type_aliases:` need `#[derive(Script)]` on a single-field tuple struct"
)]
pub trait ScriptAlias: ScriptType {
    /// The alias's descriptor.
    const DESCRIPTOR: TypeAliasDescriptor<'static>;
}

/// A free function exposed to scripting runtimes.
///
/// Implemented by `#[script]` on a free function, on a hidden type that
/// shares the function's name (and therefore travels with `use` imports and
/// re-exports).
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a `#[script]` function",
    note = "functions listed in `registry!` need the `#[script]` attribute"
)]
pub trait ScriptFunction {
    /// The function's descriptor.
    const DESCRIPTOR: FunctionDescriptor<'static>;
}

/// The methods, constructors, and properties a type exposes to scripting
/// runtimes.
///
/// Implemented by `#[script]` on an inherent `impl` block.
#[diagnostic::on_unimplemented(
    message = "`{Self}` declares `#[script(methods)]` but no `#[script] impl {Self}` block was found",
    note = "add `#[script]` to an `impl {Self}` block, or remove `methods` from `#[script(...)]` on the type"
)]
pub trait ScriptImpl {
    /// Exposed methods, including static methods.
    const METHODS: &'static [FunctionDescriptor<'static>];
    /// Constructor functions.
    const CONSTRUCTORS: &'static [FunctionDescriptor<'static>];
    /// Computed properties.
    const PROPERTIES: &'static [PropertyDescriptor<'static>];
    /// Whether any exposed function is `async`.
    const HAS_ASYNC: bool;
}
