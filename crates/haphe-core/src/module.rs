use crate::function::FunctionDescriptor;
use crate::types::{TypeDescriptor, TypeId};

/// A logical grouping of functions, types, constants, and sub-modules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModuleDescriptor<'a> {
    /// Module name.
    pub name: &'a str,
    /// Optional documentation string.
    pub doc: Option<&'a str>,
    /// Free functions in this module.
    pub functions: &'a [FunctionDescriptor<'a>],
    /// [`TypeId`]s of types that belong to this module.
    pub type_ids: &'a [TypeId<'a>],
    /// Nested sub-modules.
    pub submodules: &'a [ModuleDescriptor<'a>],
    /// Named constants exposed by this module.
    pub constants: &'a [ConstantDescriptor<'a>],
}

/// A named constant value exposed to scripting languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConstantDescriptor<'a> {
    /// Constant name.
    pub name: &'a str,
    /// Optional documentation string.
    pub doc: Option<&'a str>,
    /// The constant's type.
    pub ty: &'a TypeDescriptor<'a>,
    /// String representation of the constant's value.
    pub value: &'a str,
}
