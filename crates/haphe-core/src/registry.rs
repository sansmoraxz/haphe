use std::collections::HashSet;

use crate::function::FunctionDescriptor;
use crate::module::ModuleDescriptor;
use crate::types::{
    EnumDescriptor, StructDescriptor, TraitImpl, TypeAliasDescriptor, TypeDescriptor, TypeId,
    VariantKind,
};

/// Errors that can occur when building or validating a [`TypeRegistry`].
#[derive(Debug, Clone)]
pub enum RegistryError<'a> {
    /// A type with this [`TypeId`] is already registered.
    DuplicateType { id: TypeId<'a> },
    /// A [`TypeDescriptor::Ref`] points to a [`TypeId`] that is not registered.
    DanglingRef { from: TypeId<'a>, to: TypeId<'a> },
    /// A [`TypeDescriptor::GenericParam`] references a name not declared on
    /// the owning type.
    UndeclaredGenericParam {
        owner: TypeId<'a>,
        param_name: &'a str,
    },
    /// A module references a [`TypeId`] that is not registered — in a
    /// function signature, a constant type, or
    /// [`ModuleDescriptor::type_ids`](crate::ModuleDescriptor::type_ids).
    DanglingModuleRef { module: &'a str, to: TypeId<'a> },
    /// Two members of a type (fields, methods, constructors, or properties)
    /// share an exposed name.
    DuplicateMember { owner: TypeId<'a>, name: &'a str },
    /// Two entries in a module (functions, constants, or submodules) share an
    /// exposed name.
    DuplicateModuleEntry { module: &'a str, name: &'a str },
}

impl std::fmt::Display for RegistryError<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateType { id } => write!(f, "duplicate type registration: {id}"),
            Self::DanglingRef { from, to } => {
                write!(f, "type {from} references unregistered type {to}")
            }
            Self::UndeclaredGenericParam { owner, param_name } => {
                write!(
                    f,
                    "type {owner} uses undeclared generic parameter `{param_name}`"
                )
            }
            Self::DanglingModuleRef { module, to } => {
                write!(f, "module `{module}` references unregistered type {to}")
            }
            Self::DuplicateMember { owner, name } => {
                write!(f, "type {owner} exposes the name `{name}` more than once")
            }
            Self::DuplicateModuleEntry { module, name } => {
                write!(
                    f,
                    "module `{module}` exposes the name `{name}` more than once"
                )
            }
        }
    }
}

impl std::error::Error for RegistryError<'_> {}

/// A reference to a struct, enum, or type alias in the registry.
#[derive(Debug)]
pub enum TypeKind<'a> {
    /// A registered struct.
    Struct(&'a StructDescriptor<'a>),
    /// A registered enum.
    Enum(&'a EnumDescriptor<'a>),
    /// A registered type alias or newtype.
    TypeAlias(&'a TypeAliasDescriptor<'a>),
}

/// Const-constructible, read-only view of all type and module descriptions.
///
/// Built from static slices for compile-time definitions, or borrowed from a
/// [`TypeRegistryBuilder`] for dynamic construction. Handed to a
/// [`RuntimeBinder`](crate::RuntimeBinder) to produce scripting-language bindings.
#[derive(Debug)]
pub struct TypeRegistry<'a> {
    structs: &'a [StructDescriptor<'a>],
    enums: &'a [EnumDescriptor<'a>],
    type_aliases: &'a [TypeAliasDescriptor<'a>],
    modules: &'a [ModuleDescriptor<'a>],
}

impl<'a> TypeRegistry<'a> {
    /// Creates a registry from pre-built slices. Usable in `const` contexts.
    pub const fn new(
        structs: &'a [StructDescriptor<'a>],
        enums: &'a [EnumDescriptor<'a>],
        type_aliases: &'a [TypeAliasDescriptor<'a>],
        modules: &'a [ModuleDescriptor<'a>],
    ) -> Self {
        Self {
            structs,
            enums,
            type_aliases,
            modules,
        }
    }

    /// Looks up a struct by its [`TypeId`].
    pub fn get_struct(&self, id: &TypeId<'_>) -> Option<&StructDescriptor<'a>> {
        self.structs.iter().find(|s| s.id == *id)
    }

    /// Looks up an enum by its [`TypeId`].
    pub fn get_enum(&self, id: &TypeId<'_>) -> Option<&EnumDescriptor<'a>> {
        self.enums.iter().find(|e| e.id == *id)
    }

    /// Looks up a type alias by its [`TypeId`].
    pub fn get_type_alias(&self, id: &TypeId<'_>) -> Option<&TypeAliasDescriptor<'a>> {
        self.type_aliases.iter().find(|a| a.id == *id)
    }

    /// Looks up any registered type (struct, enum, or type alias) by its
    /// [`TypeId`].
    pub fn get_type(&self, id: &TypeId<'_>) -> Option<TypeKind<'_>> {
        if let Some(s) = self.get_struct(id) {
            return Some(TypeKind::Struct(s));
        }
        if let Some(e) = self.get_enum(id) {
            return Some(TypeKind::Enum(e));
        }
        if let Some(a) = self.get_type_alias(id) {
            return Some(TypeKind::TypeAlias(a));
        }
        None
    }

    /// Returns all registered structs.
    pub const fn structs(&self) -> &[StructDescriptor<'a>] {
        self.structs
    }

    /// Returns all registered enums.
    pub const fn enums(&self) -> &[EnumDescriptor<'a>] {
        self.enums
    }

    /// Returns all registered type aliases.
    pub const fn type_aliases(&self) -> &[TypeAliasDescriptor<'a>] {
        self.type_aliases
    }

    /// Returns all top-level modules.
    pub const fn modules(&self) -> &[ModuleDescriptor<'a>] {
        self.modules
    }

    /// Validates structural integrity and returns a [`ValidatedRegistry`] that
    /// can be passed to [`crate::RuntimeBinder::bind`].
    ///
    /// Checks that every [`TypeDescriptor::Ref`] points to a registered type
    /// and every [`TypeDescriptor::GenericParam`] names a declared parameter.
    pub fn validate(&'a self) -> Result<ValidatedRegistry<'a>, Vec<RegistryError<'a>>> {
        let mut errors = Vec::new();
        let mut known: HashSet<TypeId<'a>> = HashSet::new();
        for id in self
            .structs
            .iter()
            .map(|s| s.id)
            .chain(self.enums.iter().map(|e| e.id))
            .chain(self.type_aliases.iter().map(|a| a.id))
        {
            if !known.insert(id) {
                errors.push(RegistryError::DuplicateType { id });
            }
        }

        for s in self.structs {
            let generic_names: HashSet<&str> = s.generic_params.iter().map(|g| g.name).collect();

            for field in s.fields {
                collect_dangling_refs(s.id, field.ty, &known, &mut errors);
                collect_undeclared_generics(s.id, field.ty, &generic_names, &mut errors);
            }
            collect_dangling_refs_in_methods(s.id, s.methods, &known, &mut errors);
            collect_undeclared_generics_in_methods(s.id, s.methods, &generic_names, &mut errors);
            collect_dangling_refs_in_methods(s.id, s.constructors, &known, &mut errors);
            collect_undeclared_generics_in_methods(
                s.id,
                s.constructors,
                &generic_names,
                &mut errors,
            );
            for prop in s.properties {
                collect_dangling_refs(s.id, prop.ty, &known, &mut errors);
                collect_undeclared_generics(s.id, prop.ty, &generic_names, &mut errors);
            }
            collect_dangling_refs_in_trait_impls(s.id, s.trait_impls, &known, &mut errors);
            for gp in s.generic_params {
                if let Some(default) = gp.default {
                    collect_dangling_refs(s.id, default, &known, &mut errors);
                }
            }
            collect_duplicate_members(
                s.id,
                s.fields
                    .iter()
                    .map(|f| f.name)
                    .chain(s.methods.iter().map(|m| m.name))
                    .chain(s.constructors.iter().map(|c| c.name))
                    .chain(s.properties.iter().map(|p| p.name)),
                &mut errors,
            );
        }

        for e in self.enums {
            let generic_names: HashSet<&str> = e.generic_params.iter().map(|g| g.name).collect();

            for variant in e.variants {
                match &variant.kind {
                    VariantKind::Unit => {}
                    VariantKind::Tuple(types) => {
                        for ty in *types {
                            collect_dangling_refs(e.id, ty, &known, &mut errors);
                            collect_undeclared_generics(e.id, ty, &generic_names, &mut errors);
                        }
                    }
                    VariantKind::Struct(fields) => {
                        for field in *fields {
                            collect_dangling_refs(e.id, field.ty, &known, &mut errors);
                            collect_undeclared_generics(
                                e.id,
                                field.ty,
                                &generic_names,
                                &mut errors,
                            );
                        }
                    }
                }
            }
            collect_dangling_refs_in_methods(e.id, e.methods, &known, &mut errors);
            collect_undeclared_generics_in_methods(e.id, e.methods, &generic_names, &mut errors);
            collect_dangling_refs_in_trait_impls(e.id, e.trait_impls, &known, &mut errors);
            for gp in e.generic_params {
                if let Some(default) = gp.default {
                    collect_dangling_refs(e.id, default, &known, &mut errors);
                }
            }
            collect_duplicate_members(
                e.id,
                e.variants
                    .iter()
                    .map(|v| v.name)
                    .chain(e.methods.iter().map(|m| m.name)),
                &mut errors,
            );
        }

        for alias in self.type_aliases {
            collect_dangling_refs(alias.id, alias.inner, &known, &mut errors);
        }

        let mut top_level_names = HashSet::new();
        for module in self.modules {
            if !top_level_names.insert(module.name) {
                errors.push(RegistryError::DuplicateModuleEntry {
                    module: module.name,
                    name: module.name,
                });
            }
            validate_module(module, &known, &mut errors);
        }

        if errors.is_empty() {
            Ok(ValidatedRegistry(self))
        } else {
            Err(errors)
        }
    }
}

/// A registry that has passed structural validation.
///
/// Created only through [`TypeRegistry::validate`]. The type system guarantees
/// every [`TypeDescriptor::Ref`] resolves, every [`TypeDescriptor::GenericParam`]
/// is declared, and no duplicate ids exist.
///
/// [`RuntimeBinder::bind`](crate::RuntimeBinder::bind) requires this
/// type, enforcing that validation always precedes code generation.
#[derive(Debug)]
pub struct ValidatedRegistry<'a>(&'a TypeRegistry<'a>);

impl<'a> ValidatedRegistry<'a> {
    /// Returns the underlying [`TypeRegistry`].
    pub const fn registry(&self) -> &TypeRegistry<'a> {
        self.0
    }
}

impl<'a> std::ops::Deref for ValidatedRegistry<'a> {
    type Target = TypeRegistry<'a>;

    fn deref(&self) -> &TypeRegistry<'a> {
        self.0
    }
}

fn collect_duplicate_members<'a>(
    owner: TypeId<'a>,
    names: impl Iterator<Item = &'a str>,
    errors: &mut Vec<RegistryError<'a>>,
) {
    let mut seen = HashSet::new();
    for name in names {
        if !seen.insert(name) {
            errors.push(RegistryError::DuplicateMember { owner, name });
        }
    }
}

fn validate_module<'a>(
    module: &'a ModuleDescriptor<'a>,
    known: &HashSet<TypeId<'a>>,
    errors: &mut Vec<RegistryError<'a>>,
) {
    let collect = |ty: &TypeDescriptor<'a>, errors: &mut Vec<RegistryError<'a>>| {
        walk_type_refs(ty, &mut |ref_id| {
            if !known.contains(ref_id) {
                errors.push(RegistryError::DanglingModuleRef {
                    module: module.name,
                    to: *ref_id,
                });
            }
        });
    };
    for function in module.functions {
        for param in function.params {
            collect(param.ty, errors);
        }
        collect(function.return_type, errors);
    }
    for constant in module.constants {
        collect(constant.ty, errors);
    }
    for type_id in module.type_ids {
        if !known.contains(type_id) {
            errors.push(RegistryError::DanglingModuleRef {
                module: module.name,
                to: *type_id,
            });
        }
    }

    let mut seen = HashSet::new();
    for name in module
        .functions
        .iter()
        .map(|f| f.name)
        .chain(module.constants.iter().map(|c| c.name))
        .chain(module.submodules.iter().map(|m| m.name))
    {
        if !seen.insert(name) {
            errors.push(RegistryError::DuplicateModuleEntry {
                module: module.name,
                name,
            });
        }
    }

    for submodule in module.submodules {
        validate_module(submodule, known, errors);
    }
}

fn collect_dangling_refs_in_trait_impls<'a>(
    owner: TypeId<'a>,
    trait_impls: &[TraitImpl<'a>],
    known: &HashSet<TypeId<'a>>,
    errors: &mut Vec<RegistryError<'a>>,
) {
    for ti in trait_impls {
        match ti {
            TraitImpl::Add { rhs, output }
            | TraitImpl::Sub { rhs, output }
            | TraitImpl::Mul { rhs, output }
            | TraitImpl::Div { rhs, output }
            | TraitImpl::Rem { rhs, output } => {
                collect_dangling_refs(owner, rhs, known, errors);
                collect_dangling_refs(owner, output, known, errors);
            }
            TraitImpl::Neg { output } => {
                collect_dangling_refs(owner, output, known, errors);
            }
            TraitImpl::Index { index, output } | TraitImpl::IndexMut { index, output } => {
                collect_dangling_refs(owner, index, known, errors);
                collect_dangling_refs(owner, output, known, errors);
            }
            TraitImpl::Iterator { item } | TraitImpl::IntoIterator { item } => {
                collect_dangling_refs(owner, item, known, errors);
            }
            TraitImpl::Display
            | TraitImpl::Debug
            | TraitImpl::Hash
            | TraitImpl::PartialEq
            | TraitImpl::Eq
            | TraitImpl::PartialOrd
            | TraitImpl::Ord
            | TraitImpl::Clone
            | TraitImpl::Default => {}
        }
    }
}

fn collect_dangling_refs_in_methods<'a>(
    owner: TypeId<'a>,
    methods: &[FunctionDescriptor<'a>],
    known: &HashSet<TypeId<'a>>,
    errors: &mut Vec<RegistryError<'a>>,
) {
    for method in methods {
        for param in method.params {
            collect_dangling_refs(owner, param.ty, known, errors);
        }
        collect_dangling_refs(owner, method.return_type, known, errors);
    }
}

fn collect_undeclared_generics_in_methods<'a>(
    owner: TypeId<'a>,
    methods: &[FunctionDescriptor<'a>],
    declared: &HashSet<&str>,
    errors: &mut Vec<RegistryError<'a>>,
) {
    for method in methods {
        for param in method.params {
            collect_undeclared_generics(owner, param.ty, declared, errors);
        }
        collect_undeclared_generics(owner, method.return_type, declared, errors);
    }
}

fn collect_dangling_refs<'a>(
    owner: TypeId<'a>,
    ty: &TypeDescriptor<'a>,
    known: &HashSet<TypeId<'a>>,
    errors: &mut Vec<RegistryError<'a>>,
) {
    walk_type_refs(ty, &mut |ref_id| {
        if !known.contains(ref_id) {
            errors.push(RegistryError::DanglingRef {
                from: owner,
                to: *ref_id,
            });
        }
    });
}

fn collect_undeclared_generics<'a>(
    owner: TypeId<'a>,
    ty: &TypeDescriptor<'a>,
    declared: &HashSet<&str>,
    errors: &mut Vec<RegistryError<'a>>,
) {
    walk_generic_params(ty, &mut |name| {
        if !declared.contains(name) {
            errors.push(RegistryError::UndeclaredGenericParam {
                owner,
                param_name: name,
            });
        }
    });
}

fn walk_type_refs<'a, F>(ty: &TypeDescriptor<'a>, visitor: &mut F)
where
    F: FnMut(&TypeId<'a>),
{
    match ty {
        TypeDescriptor::Ref(id) => visitor(id),
        TypeDescriptor::Option(inner) | TypeDescriptor::List(inner) => {
            walk_type_refs(inner, visitor)
        }
        TypeDescriptor::Array(inner, _) => walk_type_refs(inner, visitor),
        TypeDescriptor::Map(k, v) | TypeDescriptor::Result(k, v) => {
            walk_type_refs(k, visitor);
            walk_type_refs(v, visitor);
        }
        TypeDescriptor::Tuple(elems) => {
            for elem in *elems {
                walk_type_refs(elem, visitor);
            }
        }
        TypeDescriptor::Callback {
            params,
            return_type,
        } => {
            for param in *params {
                walk_type_refs(param, visitor);
            }
            walk_type_refs(return_type, visitor);
        }
        TypeDescriptor::Primitive(_)
        | TypeDescriptor::String
        | TypeDescriptor::Bytes
        | TypeDescriptor::Unit
        | TypeDescriptor::GenericParam(_) => {}
    }
}

fn walk_generic_params<'a, F>(ty: &TypeDescriptor<'a>, visitor: &mut F)
where
    F: FnMut(&'a str),
{
    match ty {
        TypeDescriptor::GenericParam(name) => visitor(name),
        TypeDescriptor::Option(inner) | TypeDescriptor::List(inner) => {
            walk_generic_params(inner, visitor)
        }
        TypeDescriptor::Array(inner, _) => walk_generic_params(inner, visitor),
        TypeDescriptor::Map(k, v) | TypeDescriptor::Result(k, v) => {
            walk_generic_params(k, visitor);
            walk_generic_params(v, visitor);
        }
        TypeDescriptor::Tuple(elems) => {
            for elem in *elems {
                walk_generic_params(elem, visitor);
            }
        }
        TypeDescriptor::Callback {
            params,
            return_type,
        } => {
            for param in *params {
                walk_generic_params(param, visitor);
            }
            walk_generic_params(return_type, visitor);
        }
        TypeDescriptor::Primitive(_)
        | TypeDescriptor::String
        | TypeDescriptor::Bytes
        | TypeDescriptor::Unit
        | TypeDescriptor::Ref(_) => {}
    }
}

/// Dynamic builder for constructing a [`TypeRegistry`] at runtime.
///
/// Stores descriptors in `Vec`s and checks for duplicate type ids on insertion.
/// Call [`as_registry`](TypeRegistryBuilder::as_registry) to borrow the
/// contents as a [`TypeRegistry`].
pub struct TypeRegistryBuilder<'a> {
    structs: Vec<StructDescriptor<'a>>,
    enums: Vec<EnumDescriptor<'a>>,
    type_aliases: Vec<TypeAliasDescriptor<'a>>,
    modules: Vec<ModuleDescriptor<'a>>,
}

impl<'a> TypeRegistryBuilder<'a> {
    /// Creates an empty builder.
    pub const fn new() -> Self {
        Self {
            structs: Vec::new(),
            enums: Vec::new(),
            type_aliases: Vec::new(),
            modules: Vec::new(),
        }
    }

    /// Registers a struct descriptor.
    ///
    /// Returns an error if a type (struct, enum, or alias) with the same id is
    /// already registered.
    pub fn register_struct(&mut self, desc: StructDescriptor<'a>) -> Result<(), RegistryError<'a>> {
        if self.has_type(&desc.id) {
            return Err(RegistryError::DuplicateType { id: desc.id });
        }
        self.structs.push(desc);
        Ok(())
    }

    /// Registers an enum descriptor.
    ///
    /// Returns an error if a type (struct, enum, or alias) with the same id is
    /// already registered.
    pub fn register_enum(&mut self, desc: EnumDescriptor<'a>) -> Result<(), RegistryError<'a>> {
        if self.has_type(&desc.id) {
            return Err(RegistryError::DuplicateType { id: desc.id });
        }
        self.enums.push(desc);
        Ok(())
    }

    /// Registers a type alias or newtype descriptor.
    ///
    /// Returns an error if a type (struct, enum, or alias) with the same id is
    /// already registered.
    pub fn register_type_alias(
        &mut self,
        desc: TypeAliasDescriptor<'a>,
    ) -> Result<(), RegistryError<'a>> {
        if self.has_type(&desc.id) {
            return Err(RegistryError::DuplicateType { id: desc.id });
        }
        self.type_aliases.push(desc);
        Ok(())
    }

    /// Adds a top-level module.
    pub fn add_module(&mut self, module: ModuleDescriptor<'a>) {
        self.modules.push(module);
    }

    /// Borrows the builder's contents as a [`TypeRegistry`].
    pub fn as_registry(&self) -> TypeRegistry<'_> {
        TypeRegistry {
            structs: &self.structs,
            enums: &self.enums,
            type_aliases: &self.type_aliases,
            modules: &self.modules,
        }
    }

    fn has_type(&self, id: &TypeId<'_>) -> bool {
        self.structs.iter().any(|s| s.id == *id)
            || self.enums.iter().any(|e| e.id == *id)
            || self.type_aliases.iter().any(|a| a.id == *id)
    }
}

impl Default for TypeRegistryBuilder<'_> {
    fn default() -> Self {
        Self::new()
    }
}

/// Implemented by Rust types to describe themselves into a
/// [`TypeRegistryBuilder`].
///
/// Each type declares its structure once, and every [`crate::RuntimeBinder`]
/// backend can consume it.
///
/// # Example
///
/// ```
/// use haphe_core::{Describe, TypeRegistryBuilder, StructDescriptor,
///                  FieldDescriptor, TypeId, TypeDescriptor, PrimitiveType,
///                  ThreadSafety, Ownership};
///
/// struct Point;
///
/// static F64_TYPE: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::F64);
/// static POINT_FIELDS: [FieldDescriptor<'static>; 2] = [
///     FieldDescriptor { name: "x", doc: None, ty: &F64_TYPE, readonly: false },
///     FieldDescriptor { name: "y", doc: None, ty: &F64_TYPE, readonly: false },
/// ];
///
/// impl Describe for Point {
///     fn describe(builder: &mut TypeRegistryBuilder<'static>) {
///         builder.register_struct(StructDescriptor {
///             id: TypeId::new("Point"),
///             name: "Point",
///             doc: Some("A 2D point"),
///             fields: &POINT_FIELDS,
///             methods: &[],
///             constructors: &[],
///             properties: &[],
///             trait_impls: &[],
///             thread_safety: ThreadSafety::SEND_SYNC,
///             generic_params: &[],
///         }).unwrap();
///     }
/// }
/// ```
pub trait Describe {
    /// Registers this type's structure in the given builder.
    fn describe(builder: &mut TypeRegistryBuilder<'static>);
}
