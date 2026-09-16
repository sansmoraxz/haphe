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
    /// Concrete instantiations of this module's generic functions, declared
    /// at the registry level (`functions: [echo<i64>]`).
    ///
    /// A second source alongside the fn-site declarations recorded in
    /// [`FunctionDescriptor::instantiations`]: descriptor-driven artifact
    /// generation can rely on registry-level entries alone, without the
    /// exposed function carrying any instantiation attributes. Live host
    /// dispatch of a monomorphized wrapper still requires the fn-site
    /// declaration, which is what compiles the wrapper.
    pub function_instantiations: &'a [FnInstantiation<'a>],
}

/// A registry-level instantiation of a generic module function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FnInstantiation<'a> {
    /// Name of the function within the module.
    pub function: &'a str,
    /// Concrete type arguments, in declaration order.
    pub args: &'a [TypeDescriptor<'a>],
}

/// Unions a function's declared instantiations with the module-level ones
/// naming it, in that order, deduplicated by type-argument equality.
pub fn union_instantiations<'a>(
    function: &'a FunctionDescriptor<'a>,
    module_level: &'a [FnInstantiation<'a>],
) -> impl Iterator<Item = &'a [TypeDescriptor<'a>]> {
    let mut seen: Vec<&'a [TypeDescriptor<'a>]> = Vec::new();
    function
        .instantiations
        .iter()
        .copied()
        .chain(
            module_level
                .iter()
                .filter(move |inst| inst.function == function.name)
                .map(|inst| inst.args),
        )
        .filter(move |args| {
            if seen.contains(args) {
                false
            } else {
                seen.push(args);
                true
            }
        })
}

impl<'a> ModuleDescriptor<'a> {
    /// All concrete instantiations of `function`, from both sources: the
    /// fn-site declarations first, then this module's registry-level entries,
    /// deduplicated by type-argument equality.
    pub fn instantiations_of(
        &'a self,
        function: &'a FunctionDescriptor<'a>,
    ) -> impl Iterator<Item = &'a [TypeDescriptor<'a>]> {
        union_instantiations(function, self.function_instantiations)
    }
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
