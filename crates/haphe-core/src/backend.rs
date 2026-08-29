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

    /// The name of the target scripting language (e.g. `"lua"`, `"rhai"`).
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
    fn generate(
        &self,
        registry: &ValidatedRegistry<'_>,
    ) -> Result<GeneratedOutput, Self::Error>;
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
    /// Supports generic type parameters on struct/enum descriptors.
    pub generics: bool,
    /// Supports computed properties on struct descriptors.
    pub properties: bool,
    /// Supports `TypeAliasDescriptor` entries in the registry.
    pub type_aliases: bool,
    /// Minimum thread safety required for all registered types.
    /// `None` = no constraint.
    pub required_thread_safety: Option<ThreadSafety>,
}

impl BackendCapabilities {
    /// All features supported, no thread-safety constraint.
    pub const ALL: Self = Self {
        async_fns: true,
        callbacks: true,
        generics: true,
        properties: true,
        type_aliases: true,
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

    /// Sets whether generic type parameters are supported.
    pub const fn with_generics(mut self, v: bool) -> Self {
        self.generics = v;
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
            }
            if !self.generics && !s.generic_params.is_empty() {
                errors.push(CompatibilityError::UnsupportedGenerics { type_id: s.id });
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
            if !self.callbacks {
                check_fns_callbacks(e.id, e.methods, &mut errors);
            }
            if !self.generics && !e.generic_params.is_empty() {
                errors.push(CompatibilityError::UnsupportedGenerics { type_id: e.id });
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

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
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

fn contains_callback(ty: &TypeDescriptor<'_>) -> bool {
    match ty {
        TypeDescriptor::Callback { .. } => true,
        TypeDescriptor::Option(inner) | TypeDescriptor::List(inner) => contains_callback(inner),
        TypeDescriptor::Array(inner, _) => contains_callback(inner),
        TypeDescriptor::Map(k, v) | TypeDescriptor::Result(k, v) => {
            contains_callback(k) || contains_callback(v)
        }
        TypeDescriptor::Tuple(elems) => elems.iter().any(contains_callback),
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
    /// A generic type in a backend that doesn't support generics.
    UnsupportedGenerics { type_id: TypeId<'a> },
    /// A computed property in a backend that doesn't support properties.
    UnsupportedProperties { type_id: TypeId<'a> },
    /// A type alias in a backend that doesn't support type aliases.
    UnsupportedTypeAlias { type_id: TypeId<'a> },
    /// A type doesn't meet the backend's thread-safety requirement.
    InsufficientThreadSafety {
        type_id: TypeId<'a>,
        required: ThreadSafety,
        actual: ThreadSafety,
    },
}

impl std::fmt::Display for CompatibilityError<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedAsync { type_id, fn_name } => {
                write!(f, "type {type_id}: async function `{fn_name}` is not supported by this backend")
            }
            Self::UnsupportedCallback { type_id, context } => {
                write!(f, "type {type_id}: callback in `{context}` is not supported by this backend")
            }
            Self::UnsupportedGenerics { type_id } => {
                write!(f, "type {type_id}: generic type parameters are not supported by this backend")
            }
            Self::UnsupportedProperties { type_id } => {
                write!(f, "type {type_id}: computed properties are not supported by this backend")
            }
            Self::UnsupportedTypeAlias { type_id } => {
                write!(f, "type alias {type_id} is not supported by this backend")
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
