pub use haphe_core::*;

/// Errors from the [`bind`] pipeline.
#[derive(Debug)]
pub enum BindError<E: std::error::Error> {
    /// The registry failed structural validation.
    Invalid(Vec<haphe_core::RegistryError<'static>>),
    /// The registry uses IR features the backend doesn't support.
    Incompatible(Vec<haphe_core::CompatibilityError<'static>>),
    /// The backend returned an error during binding.
    Bind(E),
}

impl<E: std::error::Error> std::fmt::Display for BindError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(errors) => {
                write!(f, "registry validation failed ({} errors)", errors.len())
            }
            Self::Incompatible(errors) => {
                write!(f, "backend compatibility check failed ({} errors)", errors.len())
            }
            Self::Bind(e) => write!(f, "runtime binding failed: {e}"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for BindError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Bind(e) => Some(e),
            _ => None,
        }
    }
}

/// Validates, checks compatibility, and binds types into a runtime.
pub fn bind<B: RuntimeBinder>(
    binder: &B,
    registry: &'static TypeRegistry<'static>,
    runtime: &mut B::Runtime,
) -> Result<(), BindError<B::Error>> {
    let validated = registry
        .validate()
        .map_err(BindError::Invalid)?;

    binder
        .capabilities()
        .check(&validated)
        .map_err(BindError::Incompatible)?;

    binder
        .bind(&validated, runtime)
        .map_err(BindError::Bind)
}

/// Errors from the [`generate`] pipeline.
#[derive(Debug)]
pub enum GenerateError<E: std::error::Error> {
    /// The registry failed structural validation.
    Invalid(Vec<haphe_core::RegistryError<'static>>),
    /// The registry uses IR features the backend doesn't support.
    Incompatible(Vec<haphe_core::CompatibilityError<'static>>),
    /// The backend returned an error during generation.
    Backend(E),
}

impl<E: std::error::Error> std::fmt::Display for GenerateError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(errors) => {
                write!(f, "registry validation failed ({} errors)", errors.len())
            }
            Self::Incompatible(errors) => {
                write!(f, "backend compatibility check failed ({} errors)", errors.len())
            }
            Self::Backend(e) => write!(f, "binding generation failed: {e}"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for GenerateError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Backend(e) => Some(e),
            _ => None,
        }
    }
}

/// Validates, checks compatibility, and generates binding artifacts.
/// For example, this can be used to generate pyi files for Python
/// or d.ts files for TypeScript.
pub fn generate<G: BindingGenerator>(
    generator: &G,
    registry: &'static TypeRegistry<'static>,
) -> Result<GeneratedOutput, GenerateError<G::Error>> {
    let validated = registry
        .validate()
        .map_err(GenerateError::Invalid)?;

    generator
        .capabilities()
        .check(&validated)
        .map_err(GenerateError::Incompatible)?;

    generator
        .generate(&validated)
        .map_err(GenerateError::Backend)
}
