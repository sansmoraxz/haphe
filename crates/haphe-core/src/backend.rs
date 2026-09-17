use crate::registry::ValidatedRegistry;
use crate::types::{ThreadSafety, TypeDescriptor, TypeId};

/// Trait for registering described types into an embedded scripting runtime.
///
/// This is the primary backend interface. Each implementation bridges the IR
/// to a specific runtime's registration API. The trait is monomorphized per
/// backend.
pub trait RuntimeBinder {
    /// The scripting runtime type.
    type Runtime: ?Sized;
    /// The error type returned during binding.
    type Error: std::error::Error;

    /// The name of the target scripting language.
    fn language_name(&self) -> &str;

    /// Declares which IR features this backend supports and what constraints
    /// it imposes.
    fn capabilities(&self) -> BackendCapabilities;

    /// Registers all described types and functions from the registry into
    /// the runtime.
    fn bind(
        &self,
        registry: &ValidatedRegistry<'_>,
        runtime: &mut Self::Runtime,
    ) -> Result<(), Self::Error>;
}

/// Trait for generating binding artifact files from described types.
///
/// Produces files like `.pyi` (Python type stubs), `.d.ts` (TypeScript
/// declarations), or documentation for editor tooling. For registering
/// types into a live embedded runtime, see [`RuntimeBinder`].
pub trait BindingGenerator {
    /// The error type returned by this generator.
    type Error: std::error::Error;

    /// The name of the target language (e.g. `"python"`, `"typescript"`).
    fn language_name(&self) -> &str;

    /// Declares which IR features this generator handles.
    fn capabilities(&self) -> BackendCapabilities;

    /// Consumes a validated type registry and produces generated files.
    fn generate(&self, registry: &ValidatedRegistry<'_>) -> Result<GeneratedOutput, Self::Error>;
}

/// IR features a backend can handle and constraints it imposes.
///
/// Used by both [`RuntimeBinder`] and [`BindingGenerator`]. Call
/// [`check`](BackendCapabilities::check) to validate a registry against
/// these capabilities before binding or generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct BackendCapabilities {
    /// Supports `is_async: true` on functions.
    pub async_fns: bool,
    /// Supports `TypeDescriptor::Callback` in fields/params.
    pub callbacks: bool,
    /// Supports `TypeDescriptor::Stream` values.
    pub streams: bool,
    /// Supports `TypeDescriptor::Future` values.
    pub futures: bool,
    /// Supports generic type parameters on struct/enum descriptors.
    pub generics: bool,
    /// Supports `dyn`-dispatched generic functions: an OPT-IN capability a
    /// backend satisfies natively (a dynamically-typed runtime) or by
    /// injecting synthesized dispatch machinery behind a feature flag —
    /// report `true` only when that machinery is compiled in.
    pub dyn_generics: bool,
    /// Supports computed properties on struct descriptors.
    pub properties: bool,
    /// Supports `TypeAliasDescriptor` entries in the registry.
    pub type_aliases: bool,
    /// Supports `ForeignInterfaceDescriptor` entries in the registry.
    pub foreign_fns: bool,
    /// Minimum thread safety required for all registered types.
    /// `None` = no constraint.
    pub required_thread_safety: Option<ThreadSafety>,
}

impl BackendCapabilities {
    /// All features supported, no thread-safety constraint.
    pub const ALL: Self = Self {
        async_fns: true,
        callbacks: true,
        streams: true,
        futures: true,
        generics: true,
        dyn_generics: true,
        properties: true,
        type_aliases: true,
        foreign_fns: true,
        required_thread_safety: None,
    };

    /// Sets whether async functions are supported.
    pub const fn with_async_fns(mut self, v: bool) -> Self {
        self.async_fns = v;
        self
    }

    /// Sets whether callbacks are supported.
    pub const fn with_callbacks(mut self, v: bool) -> Self {
        self.callbacks = v;
        self
    }

    /// Sets whether stream value types are supported.
    pub const fn with_streams(mut self, v: bool) -> Self {
        self.streams = v;
        self
    }

    /// Sets whether future value types are supported.
    pub const fn with_futures(mut self, v: bool) -> Self {
        self.futures = v;
        self
    }

    /// Sets whether generic type parameters are supported.
    pub const fn with_generics(mut self, v: bool) -> Self {
        self.generics = v;
        self
    }

    /// Sets whether `dyn`-dispatched generic functions are supported.
    pub const fn with_dyn_generics(mut self, v: bool) -> Self {
        self.dyn_generics = v;
        self
    }

    /// Sets whether computed properties are supported.
    pub const fn with_properties(mut self, v: bool) -> Self {
        self.properties = v;
        self
    }

    /// Sets whether type aliases are supported.
    pub const fn with_type_aliases(mut self, v: bool) -> Self {
        self.type_aliases = v;
        self
    }

    /// Sets whether foreign interfaces are supported.
    pub const fn with_foreign_fns(mut self, v: bool) -> Self {
        self.foreign_fns = v;
        self
    }

    /// Sets the minimum thread safety required for all registered types.
    pub const fn with_required_thread_safety(mut self, v: Option<ThreadSafety>) -> Self {
        self.required_thread_safety = v;
        self
    }

    /// Checks a validated registry against these capabilities.
    ///
    /// Returns `Ok(())` if the registry is compatible, or a list of
    /// [`CompatibilityError`]s describing every mismatch.
    pub fn check<'a>(
        &self,
        registry: &ValidatedRegistry<'a>,
    ) -> Result<(), Vec<CompatibilityError<'a>>> {
        let mut errors = Vec::new();

        for s in registry.structs() {
            check_fns_async(s.id, s.methods, self.async_fns, &mut errors);
            check_fns_async(s.id, s.constructors, self.async_fns, &mut errors);
            check_trait_impls_async(s.id, s.trait_impls, self.async_fns, &mut errors);
            check_fns_generics(
                s.id,
                s.methods,
                self.generics,
                self.dyn_generics,
                &mut errors,
            );
            check_fns_generics(
                s.id,
                s.constructors,
                self.generics,
                self.dyn_generics,
                &mut errors,
            );
            if !self.callbacks {
                check_fns_callbacks(s.id, s.methods, &mut errors);
                check_fns_callbacks(s.id, s.constructors, &mut errors);
                for field in s.fields {
                    if contains_callback(field.ty) {
                        errors.push(CompatibilityError::UnsupportedCallback {
                            type_id: s.id,
                            context: field.name,
                        });
                    }
                }
                for prop in s.properties {
                    if contains_callback(prop.ty) {
                        errors.push(CompatibilityError::UnsupportedCallback {
                            type_id: s.id,
                            context: prop.name,
                        });
                    }
                }
            }
            if !self.streams {
                check_fns_streams(s.id, s.methods, &mut errors);
                check_fns_streams(s.id, s.constructors, &mut errors);
                for field in s.fields {
                    if contains_stream(field.ty) {
                        errors.push(CompatibilityError::UnsupportedStream {
                            type_id: s.id,
                            context: field.name,
                        });
                    }
                }
                for prop in s.properties {
                    if contains_stream(prop.ty) {
                        errors.push(CompatibilityError::UnsupportedStream {
                            type_id: s.id,
                            context: prop.name,
                        });
                    }
                }
            }
            if !self.futures {
                check_fns_futures(s.id, s.methods, &mut errors);
                check_fns_futures(s.id, s.constructors, &mut errors);
                for field in s.fields {
                    if contains_future(field.ty) {
                        errors.push(CompatibilityError::UnsupportedFuture {
                            type_id: s.id,
                            context: field.name,
                        });
                    }
                }
                for prop in s.properties {
                    if contains_future(prop.ty) {
                        errors.push(CompatibilityError::UnsupportedFuture {
                            type_id: s.id,
                            context: prop.name,
                        });
                    }
                }
            }
            if !s.generic_params.is_empty() {
                if !self.generics {
                    errors.push(CompatibilityError::UnsupportedGenerics { type_id: s.id });
                } else if !registry.instantiations().iter().any(|i| i.id == s.id) {
                    errors.push(CompatibilityError::UninstantiatedGeneric { type_id: s.id });
                }
            }
            if !self.properties && !s.properties.is_empty() {
                errors.push(CompatibilityError::UnsupportedProperties { type_id: s.id });
            }
            if let Some(required) = self.required_thread_safety
                && !meets_thread_safety(&s.thread_safety, &required)
            {
                errors.push(CompatibilityError::InsufficientThreadSafety {
                    type_id: s.id,
                    required,
                    actual: s.thread_safety,
                });
            }
        }

        for e in registry.enums() {
            check_fns_async(e.id, e.methods, self.async_fns, &mut errors);
            check_trait_impls_async(e.id, e.trait_impls, self.async_fns, &mut errors);
            check_fns_generics(
                e.id,
                e.methods,
                self.generics,
                self.dyn_generics,
                &mut errors,
            );
            if !self.callbacks {
                check_fns_callbacks(e.id, e.methods, &mut errors);
            }
            if !self.streams {
                check_fns_streams(e.id, e.methods, &mut errors);
                for v in e.variants {
                    if variant_payload_matches(&v.kind, contains_stream) {
                        errors.push(CompatibilityError::UnsupportedStream {
                            type_id: e.id,
                            context: v.name,
                        });
                    }
                }
            }
            if !self.futures {
                check_fns_futures(e.id, e.methods, &mut errors);
                for v in e.variants {
                    if variant_payload_matches(&v.kind, contains_future) {
                        errors.push(CompatibilityError::UnsupportedFuture {
                            type_id: e.id,
                            context: v.name,
                        });
                    }
                }
            }
            if !e.generic_params.is_empty() {
                if !self.generics {
                    errors.push(CompatibilityError::UnsupportedGenerics { type_id: e.id });
                } else if !registry.instantiations().iter().any(|i| i.id == e.id) {
                    errors.push(CompatibilityError::UninstantiatedGeneric { type_id: e.id });
                }
            }
            if let Some(required) = self.required_thread_safety
                && !meets_thread_safety(&e.thread_safety, &required)
            {
                errors.push(CompatibilityError::InsufficientThreadSafety {
                    type_id: e.id,
                    required,
                    actual: e.thread_safety,
                });
            }
        }

        if !self.type_aliases {
            for a in registry.type_aliases() {
                errors.push(CompatibilityError::UnsupportedTypeAlias { type_id: a.id });
            }
        }

        for fi in registry.foreign_interfaces() {
            if !self.foreign_fns {
                errors.push(CompatibilityError::UnsupportedForeignInterface { type_id: fi.id });
                continue;
            }
            check_fns_async(fi.id, fi.functions, self.async_fns, &mut errors);
            if !self.callbacks {
                check_fns_callbacks(fi.id, fi.functions, &mut errors);
            }
            if !self.streams {
                check_fns_streams(fi.id, fi.functions, &mut errors);
            }
            if !self.futures {
                check_fns_futures(fi.id, fi.functions, &mut errors);
            }
            if !fi.generic_params.is_empty() {
                if !self.generics {
                    errors.push(CompatibilityError::UnsupportedGenerics { type_id: fi.id });
                } else if !registry.instantiations().iter().any(|i| i.id == fi.id) {
                    errors.push(CompatibilityError::UninstantiatedGeneric { type_id: fi.id });
                }
            }
            check_fns_generics(
                fi.id,
                fi.functions,
                self.generics,
                self.dyn_generics,
                &mut errors,
            );
            if let Some(required) = self.required_thread_safety
                && !meets_thread_safety(&fi.thread_safety, &required)
            {
                errors.push(CompatibilityError::InsufficientThreadSafety {
                    type_id: fi.id,
                    required,
                    actual: fi.thread_safety,
                });
            }
        }

        for module in registry.modules() {
            self.check_module(module, &mut errors);
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    fn check_module<'a>(
        &self,
        module: &crate::module::ModuleDescriptor<'a>,
        errors: &mut Vec<CompatibilityError<'a>>,
    ) {
        for function in module.functions {
            if !self.async_fns && function.is_async {
                errors.push(CompatibilityError::UnsupportedModuleAsync {
                    module: module.name,
                    fn_name: function.name,
                });
            }
            if !function.generic_params.is_empty() {
                if matches!(function.dispatch, crate::function::Dispatch::Dyn) && !self.dyn_generics
                {
                    errors.push(CompatibilityError::DynGenericsUnsupported {
                        function: function.name,
                    });
                }
                if !self.generics {
                    errors.push(CompatibilityError::UnsupportedModuleGenerics {
                        module: module.name,
                        fn_name: function.name,
                    });
                } else if module.instantiations_of(function).next().is_none() {
                    errors.push(CompatibilityError::UninstantiatedModuleGeneric {
                        module: module.name,
                        fn_name: function.name,
                    });
                }
            }
            if !self.callbacks {
                let has_callback = function.params.iter().any(|p| contains_callback(p.ty))
                    || contains_callback(function.return_type);
                if has_callback {
                    errors.push(CompatibilityError::UnsupportedModuleCallback {
                        module: module.name,
                        context: function.name,
                    });
                }
            }
            if !self.streams {
                let has_stream = function.params.iter().any(|p| contains_stream(p.ty))
                    || contains_stream(function.return_type);
                if has_stream {
                    errors.push(CompatibilityError::UnsupportedModuleStream {
                        module: module.name,
                        context: function.name,
                    });
                }
            }
            if !self.futures {
                let has_future = function.params.iter().any(|p| contains_future(p.ty))
                    || contains_future(function.return_type);
                if has_future {
                    errors.push(CompatibilityError::UnsupportedModuleFuture {
                        module: module.name,
                        context: function.name,
                    });
                }
            }
        }
        if !self.callbacks {
            for constant in module.constants {
                if contains_callback(constant.ty) {
                    errors.push(CompatibilityError::UnsupportedModuleCallback {
                        module: module.name,
                        context: constant.name,
                    });
                }
            }
        }
        if !self.streams {
            for constant in module.constants {
                if contains_stream(constant.ty) {
                    errors.push(CompatibilityError::UnsupportedModuleStream {
                        module: module.name,
                        context: constant.name,
                    });
                }
            }
        }
        if !self.futures {
            for constant in module.constants {
                if contains_future(constant.ty) {
                    errors.push(CompatibilityError::UnsupportedModuleFuture {
                        module: module.name,
                        context: constant.name,
                    });
                }
            }
        }
        for submodule in module.submodules {
            self.check_module(submodule, errors);
        }
    }
}

fn check_fns_async<'a>(
    type_id: TypeId<'a>,
    fns: &[crate::function::FunctionDescriptor<'a>],
    supports_async: bool,
    errors: &mut Vec<CompatibilityError<'a>>,
) {
    if supports_async {
        return;
    }
    for f in fns {
        if f.is_async {
            errors.push(CompatibilityError::UnsupportedAsync {
                type_id,
                fn_name: f.name,
            });
        }
    }
}

fn check_trait_impls_async<'a>(
    type_id: TypeId<'a>,
    trait_impls: &[crate::types::TraitImpl<'a>],
    supports_async: bool,
    errors: &mut Vec<CompatibilityError<'a>>,
) {
    if supports_async {
        return;
    }
    for ti in trait_impls {
        if matches!(ti, crate::types::TraitImpl::AsyncCall { .. }) {
            errors.push(CompatibilityError::UnsupportedAsync {
                type_id,
                fn_name: "call",
            });
        }
    }
}

fn check_fns_generics<'a>(
    type_id: TypeId<'a>,
    fns: &'a [crate::function::FunctionDescriptor<'a>],
    supports_generics: bool,
    dyn_generics: bool,
    errors: &mut Vec<CompatibilityError<'a>>,
) {
    for f in fns {
        if f.generic_params.is_empty() {
            continue;
        }
        if matches!(f.dispatch, crate::function::Dispatch::Dyn) && !dyn_generics {
            errors.push(CompatibilityError::DynGenericsUnsupported { function: f.name });
        }
        if !supports_generics {
            errors.push(CompatibilityError::UnsupportedGenerics { type_id });
        } else if f.instantiations.is_empty() {
            errors.push(CompatibilityError::UninstantiatedGeneric { type_id });
        }
    }
}

fn check_fns_callbacks<'a>(
    type_id: TypeId<'a>,
    fns: &[crate::function::FunctionDescriptor<'a>],
    errors: &mut Vec<CompatibilityError<'a>>,
) {
    for f in fns {
        for param in f.params {
            if contains_callback(param.ty) {
                errors.push(CompatibilityError::UnsupportedCallback {
                    type_id,
                    context: f.name,
                });
                return;
            }
        }
        if contains_callback(f.return_type) {
            errors.push(CompatibilityError::UnsupportedCallback {
                type_id,
                context: f.name,
            });
        }
    }
}

fn check_fns_streams<'a>(
    type_id: TypeId<'a>,
    fns: &[crate::function::FunctionDescriptor<'a>],
    errors: &mut Vec<CompatibilityError<'a>>,
) {
    for f in fns {
        for param in f.params {
            if contains_stream(param.ty) {
                errors.push(CompatibilityError::UnsupportedStream {
                    type_id,
                    context: f.name,
                });
                return;
            }
        }
        if contains_stream(f.return_type) {
            errors.push(CompatibilityError::UnsupportedStream {
                type_id,
                context: f.name,
            });
        }
    }
}

fn contains_callback(ty: &TypeDescriptor<'_>) -> bool {
    match ty {
        TypeDescriptor::Callback { .. } => true,
        TypeDescriptor::Option(inner)
        | TypeDescriptor::List(inner)
        | TypeDescriptor::Stream(inner)
        | TypeDescriptor::Future(inner) => contains_callback(inner),
        TypeDescriptor::Array(inner, _) => contains_callback(inner),
        TypeDescriptor::Borrowed { inner, .. } => contains_callback(inner),
        TypeDescriptor::Map(k, v) | TypeDescriptor::Result(k, v) => {
            contains_callback(k) || contains_callback(v)
        }
        TypeDescriptor::Tuple(elems) => elems.iter().any(contains_callback),
        TypeDescriptor::Instance { args, .. } => args.iter().any(contains_callback),
        TypeDescriptor::Primitive(_)
        | TypeDescriptor::String
        | TypeDescriptor::Bytes
        | TypeDescriptor::Unit
        | TypeDescriptor::Ref(_)
        | TypeDescriptor::GenericParam(_) => false,
    }
}

fn check_fns_futures<'a>(
    type_id: TypeId<'a>,
    fns: &[crate::function::FunctionDescriptor<'a>],
    errors: &mut Vec<CompatibilityError<'a>>,
) {
    for f in fns {
        for param in f.params {
            if contains_future(param.ty) {
                errors.push(CompatibilityError::UnsupportedFuture {
                    type_id,
                    context: f.name,
                });
                return;
            }
        }
        if contains_future(f.return_type) {
            errors.push(CompatibilityError::UnsupportedFuture {
                type_id,
                context: f.name,
            });
        }
    }
}

fn variant_payload_matches(
    kind: &crate::types::VariantKind<'_>,
    pred: fn(&TypeDescriptor<'_>) -> bool,
) -> bool {
    match kind {
        crate::types::VariantKind::Unit => false,
        crate::types::VariantKind::Tuple(types) => types.iter().any(pred),
        crate::types::VariantKind::Struct(fields) => fields.iter().any(|f| pred(f.ty)),
    }
}

fn contains_future(ty: &TypeDescriptor<'_>) -> bool {
    match ty {
        TypeDescriptor::Future(_) => true,
        TypeDescriptor::Stream(inner) => contains_future(inner),
        TypeDescriptor::Option(inner) | TypeDescriptor::List(inner) => contains_future(inner),
        TypeDescriptor::Array(inner, _) => contains_future(inner),
        TypeDescriptor::Borrowed { inner, .. } => contains_future(inner),
        TypeDescriptor::Map(k, v) | TypeDescriptor::Result(k, v) => {
            contains_future(k) || contains_future(v)
        }
        TypeDescriptor::Tuple(elems) => elems.iter().any(contains_future),
        TypeDescriptor::Instance { args, .. } => args.iter().any(contains_future),
        TypeDescriptor::Callback {
            params,
            return_type,
        } => params.iter().any(contains_future) || contains_future(return_type),
        TypeDescriptor::Primitive(_)
        | TypeDescriptor::String
        | TypeDescriptor::Bytes
        | TypeDescriptor::Unit
        | TypeDescriptor::Ref(_)
        | TypeDescriptor::GenericParam(_) => false,
    }
}

fn contains_stream(ty: &TypeDescriptor<'_>) -> bool {
    match ty {
        TypeDescriptor::Stream(_) => true,
        TypeDescriptor::Future(inner) => contains_stream(inner),
        TypeDescriptor::Option(inner) | TypeDescriptor::List(inner) => contains_stream(inner),
        TypeDescriptor::Array(inner, _) => contains_stream(inner),
        TypeDescriptor::Borrowed { inner, .. } => contains_stream(inner),
        TypeDescriptor::Map(k, v) | TypeDescriptor::Result(k, v) => {
            contains_stream(k) || contains_stream(v)
        }
        TypeDescriptor::Tuple(elems) => elems.iter().any(contains_stream),
        TypeDescriptor::Instance { args, .. } => args.iter().any(contains_stream),
        TypeDescriptor::Callback {
            params,
            return_type,
        } => params.iter().any(contains_stream) || contains_stream(return_type),
        TypeDescriptor::Primitive(_)
        | TypeDescriptor::String
        | TypeDescriptor::Bytes
        | TypeDescriptor::Unit
        | TypeDescriptor::Ref(_)
        | TypeDescriptor::GenericParam(_) => false,
    }
}

fn meets_thread_safety(actual: &ThreadSafety, required: &ThreadSafety) -> bool {
    (!required.is_send || actual.is_send) && (!required.is_sync || actual.is_sync)
}

/// A mismatch between a registry entry and a backend's declared capabilities.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum CompatibilityError<'a> {
    /// A `dyn`-dispatched generic function in a backend without dynamic
    /// dispatch machinery.
    DynGenericsUnsupported { function: &'a str },
    /// An async function in a backend that doesn't support async.
    UnsupportedAsync {
        type_id: TypeId<'a>,
        fn_name: &'a str,
    },
    /// A callback type in a backend that doesn't support callbacks.
    UnsupportedCallback {
        type_id: TypeId<'a>,
        context: &'a str,
    },
    /// A stream type in a backend that doesn't support streams.
    UnsupportedStream {
        type_id: TypeId<'a>,
        context: &'a str,
    },
    /// A stream type in a module function or constant in a backend that
    /// doesn't support streams.
    UnsupportedModuleStream { module: &'a str, context: &'a str },
    /// A future type in a backend that doesn't support futures.
    UnsupportedFuture {
        type_id: TypeId<'a>,
        context: &'a str,
    },
    /// A future type in a module function or constant in a backend that
    /// doesn't support futures.
    UnsupportedModuleFuture { module: &'a str, context: &'a str },
    /// A generic type or foreign interface in a backend that doesn't support
    /// generics.
    UnsupportedGenerics { type_id: TypeId<'a> },
    /// A generic type or foreign interface with no recorded instantiation, in
    /// a backend that supports generics via monomorphization — there is
    /// nothing to emit.
    UninstantiatedGeneric { type_id: TypeId<'a> },
    /// A computed property in a backend that doesn't support properties.
    UnsupportedProperties { type_id: TypeId<'a> },
    /// A type alias in a backend that doesn't support type aliases.
    UnsupportedTypeAlias { type_id: TypeId<'a> },
    /// A foreign interface in a backend that doesn't support foreign
    /// interfaces.
    UnsupportedForeignInterface { type_id: TypeId<'a> },
    /// A type doesn't meet the backend's thread-safety requirement.
    InsufficientThreadSafety {
        type_id: TypeId<'a>,
        required: ThreadSafety,
        actual: ThreadSafety,
    },
    /// An async module function in a backend that doesn't support async.
    UnsupportedModuleAsync { module: &'a str, fn_name: &'a str },
    /// A generic module function in a backend that doesn't support generics.
    UnsupportedModuleGenerics { module: &'a str, fn_name: &'a str },
    /// A generic module function with no recorded instantiation, in a backend
    /// that supports generics via monomorphization — there is nothing to emit.
    UninstantiatedModuleGeneric { module: &'a str, fn_name: &'a str },
    /// A callback type in a module function or constant in a backend that
    /// doesn't support callbacks.
    UnsupportedModuleCallback { module: &'a str, context: &'a str },
}

impl std::fmt::Display for CompatibilityError<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DynGenericsUnsupported { function } => write!(
                f,
                "function `{function}` declares dyn dispatch, but this backend has no dynamic \
                 dispatch machinery (natively or via its feature flags)"
            ),
            Self::UnsupportedAsync { type_id, fn_name } => {
                write!(
                    f,
                    "type {type_id}: async function `{fn_name}` is not supported by this backend"
                )
            }
            Self::UnsupportedCallback { type_id, context } => {
                write!(
                    f,
                    "type {type_id}: callback in `{context}` is not supported by this backend"
                )
            }
            Self::UnsupportedStream { type_id, context } => {
                write!(
                    f,
                    "type {type_id}: stream in `{context}` is not supported by this backend"
                )
            }
            Self::UnsupportedModuleStream { module, context } => {
                write!(
                    f,
                    "module `{module}`: stream in `{context}` is not supported by this backend"
                )
            }
            Self::UnsupportedFuture { type_id, context } => {
                write!(
                    f,
                    "type {type_id}: future in `{context}` is not supported by this backend"
                )
            }
            Self::UnsupportedModuleFuture { module, context } => {
                write!(
                    f,
                    "module `{module}`: future in `{context}` is not supported by this backend"
                )
            }
            Self::UnsupportedGenerics { type_id } => {
                write!(
                    f,
                    "type {type_id}: generic type parameters are not supported by this backend"
                )
            }
            Self::UninstantiatedGeneric { type_id } => {
                write!(
                    f,
                    "generic type {type_id} has no recorded instantiation; nothing to emit"
                )
            }
            Self::UnsupportedProperties { type_id } => {
                write!(
                    f,
                    "type {type_id}: computed properties are not supported by this backend"
                )
            }
            Self::UnsupportedTypeAlias { type_id } => {
                write!(f, "type alias {type_id} is not supported by this backend")
            }
            Self::UnsupportedForeignInterface { type_id } => {
                write!(
                    f,
                    "foreign interface {type_id} is not supported by this backend"
                )
            }
            Self::InsufficientThreadSafety {
                type_id,
                required,
                actual,
            } => {
                write!(
                    f,
                    "type {type_id}: requires Send={}/Sync={} but has Send={}/Sync={}",
                    required.is_send, required.is_sync, actual.is_send, actual.is_sync
                )
            }
            Self::UnsupportedModuleAsync { module, fn_name } => {
                write!(
                    f,
                    "module `{module}`: async function `{fn_name}` is not supported by this backend"
                )
            }
            Self::UnsupportedModuleGenerics { module, fn_name } => {
                write!(
                    f,
                    "module `{module}`: generic function `{fn_name}` is not supported by this backend"
                )
            }
            Self::UninstantiatedModuleGeneric { module, fn_name } => {
                write!(
                    f,
                    "module `{module}`: generic function `{fn_name}` has no recorded instantiation; nothing to emit"
                )
            }
            Self::UnsupportedModuleCallback { module, context } => {
                write!(
                    f,
                    "module `{module}`: callback in `{context}` is not supported by this backend"
                )
            }
        }
    }
}

impl std::error::Error for CompatibilityError<'_> {}

/// The complete output of a binding-generation pass.
#[derive(Debug, Clone)]
pub struct GeneratedOutput {
    /// Generated files, each with a relative path and content.
    pub files: Vec<GeneratedFile>,
}

/// A single generated output file.
#[derive(Debug, Clone)]
pub struct GeneratedFile {
    /// Relative path for this file (e.g. `"bindings/point.pyi"`).
    pub path: String,
    /// Raw file content. Text in any encoding or binary bytecode.
    pub content: Vec<u8>,
    /// Character encoding for text output (e.g. `"utf-8"`, `"shift_jis"`).
    /// `None` indicates binary content.
    pub encoding: Option<String>,
}
