//! Describe Rust types once, bind them into any embedded scripting runtime.
//!
//! # Quickstart
//!
//! ```
//! use haphe::{Script, script};
//!
//! /// A 2D point.
//! #[derive(Script, PartialEq)]
//! #[script(thread_safety = send_sync, traits(PartialEq), methods)]
//! struct Point {
//!     x: f64,
//!     #[script(readonly)]
//!     y: f64,
//! }
//!
//! #[script]
//! impl Point {
//!     #[script(constructor)]
//!     fn new(x: f64, y: f64) -> Self { Point { x, y } }
//!
//!     fn distance_to(&self, other: &Point) -> f64 {
//!         ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
//!     }
//! }
//!
//! /// Adds two numbers.
//! #[script]
//! fn add(a: i32, b: i32) -> i32 { a + b }
//!
//! haphe::registry! {
//!     pub static REGISTRY = {
//!         structs: [Point],
//!         modules: [
//!             mod math { functions: [add], types: [Point] },
//!         ],
//!     };
//! }
//!
//! let validated = REGISTRY.validate().unwrap();
//! # let _ = validated;
//! ```

pub use haphe_core::*;

#[cfg(feature = "macros")]
pub use haphe_macros::{Script, registry, script};

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
                write!(
                    f,
                    "backend compatibility check failed ({} errors)",
                    errors.len()
                )
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
    let validated = registry.validate().map_err(BindError::Invalid)?;

    binder
        .capabilities()
        .check(&validated)
        .map_err(BindError::Incompatible)?;

    binder.bind(&validated, runtime).map_err(BindError::Bind)
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
                write!(
                    f,
                    "backend compatibility check failed ({} errors)",
                    errors.len()
                )
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
    let validated = registry.validate().map_err(GenerateError::Invalid)?;

    generator
        .capabilities()
        .check(&validated)
        .map_err(GenerateError::Incompatible)?;

    generator
        .generate(&validated)
        .map_err(GenerateError::Backend)
}
