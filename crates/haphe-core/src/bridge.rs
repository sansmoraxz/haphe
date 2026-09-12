//! Invocation bridge: connects haphe type descriptors to live runtime
//! bindings.
//!
//! The bridge has two layers:
//!
//! 1. **Value conversion** — [`IntoScript`] / [`FromScript`] convert between
//!    Rust values and a fixed [`ScriptValue`] enum. Backends convert
//!    `ScriptValue` to/from their native representation.
//!
//! 2. **Type registration** — [`ScriptBind`] hands concrete fn pointers
//!    (getters, setters, methods, constructors) to a [`TypeBinder`], which
//!    wraps them into the target runtime's API.

use std::collections::HashMap;
use std::sync::Arc;

use crate::types::TypeId;

// ---------------------------------------------------------------------------
// Value conversion
// ---------------------------------------------------------------------------

/// Type-erased wrapper for user-defined types flowing through [`ScriptValue`].
///
/// Uses `Arc` internally so that `ScriptValue` remains `Clone`.
#[derive(Clone)]
pub struct OpaqueUserData(pub Arc<dyn std::any::Any + Send + Sync>);

impl OpaqueUserData {
    pub fn new<T: Send + Sync + 'static>(value: T) -> Self {
        Self(Arc::new(value))
    }

    pub fn downcast_ref<T: 'static>(&self) -> Option<&T> {
        self.0.downcast_ref()
    }

    pub fn downcast_clone<T: Clone + 'static>(&self) -> Option<T> {
        self.0.downcast_ref::<T>().cloned()
    }
}

impl std::fmt::Debug for OpaqueUserData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("UserData")
            .field(&self.0.type_id())
            .finish()
    }
}

/// Runtime-agnostic value representation.
///
/// Covers the types every scripting runtime supports. Backends match on
/// variants to convert to/from their native value type.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ScriptValue {
    Unit,
    Bool(bool),
    I64(i64),
    F64(f64),
    String(String),
    Bytes(Vec<u8>),
    Char(char),
    List(Vec<ScriptValue>),
    Map(Vec<(String, ScriptValue)>),
    Optional(Option<Box<ScriptValue>>),
    UserData(OpaqueUserData),
}

/// Error returned when a [`ScriptValue`] variant doesn't match the expected
/// Rust type.
#[derive(Debug, Clone)]
pub struct ScriptConvertError {
    pub expected: &'static str,
    pub got: &'static str,
}

impl std::fmt::Display for ScriptConvertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "script value conversion: expected {}, got {}",
            self.expected, self.got
        )
    }
}

impl std::error::Error for ScriptConvertError {}

impl ScriptValue {
    pub fn variant_name(&self) -> &'static str {
        match self {
            Self::Unit => "unit",
            Self::Bool(_) => "bool",
            Self::I64(_) => "i64",
            Self::F64(_) => "f64",
            Self::String(_) => "string",
            Self::Bytes(_) => "bytes",
            Self::Char(_) => "char",
            Self::List(_) => "list",
            Self::Map(_) => "map",
            Self::Optional(_) => "optional",
            Self::UserData(_) => "userdata",
        }
    }
}

/// A Rust value that can be pushed into a scripting runtime.
///
/// Automatically provided for any `T` where `ScriptValue: From<T>`,
/// following the same `From`/`Into` pattern as the standard library.
/// Implement [`From<T>`] for [`ScriptValue`] instead of this trait directly.
pub trait IntoScript {
    fn into_script(self) -> ScriptValue;
}

impl<T> IntoScript for T
where
    ScriptValue: From<T>,
{
    fn into_script(self) -> ScriptValue {
        ScriptValue::from(self)
    }
}

/// A Rust value that can be extracted from a scripting runtime.
pub trait FromScript: Sized {
    fn from_script(value: ScriptValue) -> Result<Self, ScriptConvertError>;
}

// ---------------------------------------------------------------------------
// Blanket impls for primitives
// ---------------------------------------------------------------------------

impl From<bool> for ScriptValue {
    fn from(b: bool) -> Self {
        Self::Bool(b)
    }
}

impl FromScript for bool {
    fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
        match v {
            ScriptValue::Bool(b) => Ok(b),
            other => Err(ScriptConvertError {
                expected: "bool",
                got: other.variant_name(),
            }),
        }
    }
}

macro_rules! impl_int_bridge {
    ($($ty:ty),*) => {
        $(
            impl From<$ty> for ScriptValue {
                fn from(n: $ty) -> Self {
                    Self::I64(n as i64)
                }
            }
            impl FromScript for $ty {
                fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
                    match v {
                        ScriptValue::I64(n) => Ok(n as $ty),
                        other => Err(ScriptConvertError {
                            expected: stringify!($ty),
                            got: other.variant_name(),
                        }),
                    }
                }
            }
        )*
    };
}

impl_int_bridge!(i8, i16, i32, i64, u8, u16, u32, u64);

macro_rules! impl_float_bridge {
    ($($ty:ty),*) => {
        $(
            impl From<$ty> for ScriptValue {
                fn from(n: $ty) -> Self {
                    Self::F64(n as f64)
                }
            }
            impl FromScript for $ty {
                fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
                    match v {
                        ScriptValue::F64(n) => Ok(n as $ty),
                        // Also accept integers as floats.
                        ScriptValue::I64(n) => Ok(n as $ty),
                        other => Err(ScriptConvertError {
                            expected: stringify!($ty),
                            got: other.variant_name(),
                        }),
                    }
                }
            }
        )*
    };
}

impl_float_bridge!(f32, f64);

impl From<char> for ScriptValue {
    fn from(c: char) -> Self {
        Self::Char(c)
    }
}

impl FromScript for char {
    fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
        match v {
            ScriptValue::Char(c) => Ok(c),
            ScriptValue::String(s) => s.chars().next().ok_or(ScriptConvertError {
                expected: "char",
                got: "empty string",
            }),
            other => Err(ScriptConvertError {
                expected: "char",
                got: other.variant_name(),
            }),
        }
    }
}

impl From<String> for ScriptValue {
    fn from(s: String) -> Self {
        Self::String(s)
    }
}

impl FromScript for String {
    fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
        match v {
            ScriptValue::String(s) => Ok(s),
            other => Err(ScriptConvertError {
                expected: "string",
                got: other.variant_name(),
            }),
        }
    }
}

impl From<()> for ScriptValue {
    fn from(_: ()) -> Self {
        Self::Unit
    }
}

impl FromScript for () {
    fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
        match v {
            ScriptValue::Unit => Ok(()),
            other => Err(ScriptConvertError {
                expected: "unit",
                got: other.variant_name(),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Compound type conversions
// ---------------------------------------------------------------------------

impl<T> From<Vec<T>> for ScriptValue
where
    ScriptValue: From<T>,
{
    fn from(v: Vec<T>) -> Self {
        Self::List(v.into_iter().map(ScriptValue::from).collect())
    }
}

impl<T: FromScript> FromScript for Vec<T> {
    fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
        match v {
            ScriptValue::List(list) => list.into_iter().map(T::from_script).collect(),
            other => Err(ScriptConvertError {
                expected: "list",
                got: other.variant_name(),
            }),
        }
    }
}

impl<V> From<HashMap<String, V>> for ScriptValue
where
    ScriptValue: From<V>,
{
    fn from(m: HashMap<String, V>) -> Self {
        Self::Map(m.into_iter().map(|(k, v)| (k, ScriptValue::from(v))).collect())
    }
}

impl<V: FromScript> FromScript for HashMap<String, V> {
    fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
        match v {
            ScriptValue::Map(pairs) => pairs
                .into_iter()
                .map(|(k, v)| V::from_script(v).map(|val| (k, val)))
                .collect(),
            other => Err(ScriptConvertError {
                expected: "map",
                got: other.variant_name(),
            }),
        }
    }
}

impl<T> From<Option<T>> for ScriptValue
where
    ScriptValue: From<T>,
{
    fn from(opt: Option<T>) -> Self {
        Self::Optional(opt.map(|v| Box::new(ScriptValue::from(v))))
    }
}

impl<T: FromScript> FromScript for Option<T> {
    fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
        match v {
            ScriptValue::Optional(None) | ScriptValue::Unit => Ok(None),
            ScriptValue::Optional(Some(inner)) => T::from_script(*inner).map(Some),
            other => T::from_script(other).map(Some),
        }
    }
}

// ---------------------------------------------------------------------------
// Type registration bridge
// ---------------------------------------------------------------------------

/// A type that can register its fields, methods, and constructors into a
/// scripting runtime via a [`TypeBinder`].
///
/// Implemented by `#[derive(Script)]` + `#[script] impl`. The generated
/// code hands concrete fn pointers to the binder, which wraps them into
/// the target runtime's API.
pub trait ScriptBind: Sized {
    /// Register all fields, methods, constructors, and trait metamethods.
    fn bind<B: TypeBinder<Self>>(binder: &mut B) -> Result<(), B::Error>;
}

/// Backend-provided registrar for a specific Rust type `T`.
///
/// Each method receives a concrete fn pointer with fully monomorphized
/// types. The backend wraps it into the runtime's registration API using
/// its own conversion traits. [`IntoScript`] / [`FromScript`] bounds
/// appear only on `field` (single-value primitive fields); methods use
/// bare `'static` bounds because the backend handles multi-arg conversion
/// natively.
pub trait TypeBinder<T>: Sized {
    /// The error type returned during registration.
    type Error: std::error::Error;

    /// Register a field getter and optional setter.
    fn field<V: IntoScript + FromScript + Clone + 'static>(
        &mut self,
        name: &'static str,
        getter: fn(&T) -> V,
        setter: Option<fn(&mut T, V)>,
    ) -> Result<(), Self::Error>;

    /// Register a `&self` method. The wrapper converts arguments from
    /// `ScriptValue` and returns the result as `ScriptValue` — the macro
    /// generates the conversion code with concrete types.
    fn method_ref(
        &mut self,
        name: &'static str,
        f: fn(&T, &[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>,
    ) -> Result<(), Self::Error>;

    /// Register a `&mut self` method.
    fn method_mut(
        &mut self,
        name: &'static str,
        f: fn(&mut T, &[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>,
    ) -> Result<(), Self::Error>;

    /// Register a consuming `self` method.
    fn method_owned(
        &mut self,
        name: &'static str,
        f: fn(T, &[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>,
    ) -> Result<(), Self::Error>;

    /// Register a constructor (no receiver, returns `T`).
    fn constructor(
        &mut self,
        name: &'static str,
        f: fn(&[ScriptValue]) -> Result<T, ScriptConvertError>,
    ) -> Result<(), Self::Error>;

    /// Register a `Display` / `__tostring` metamethod.
    fn meta_tostring(&mut self, f: fn(&T) -> String) -> Result<(), Self::Error>;

    /// Register a `PartialEq` / `__eq` metamethod.
    fn meta_eq(&mut self, f: fn(&T, &T) -> bool) -> Result<(), Self::Error>;

    /// Register a `__lt` metamethod.
    fn meta_lt(&mut self, f: fn(&T, &T) -> bool) -> Result<(), Self::Error>;

    /// Register a `__le` metamethod.
    fn meta_le(&mut self, f: fn(&T, &T) -> bool) -> Result<(), Self::Error>;

    /// Register a unary negation metamethod.
    fn meta_unm(&mut self, f: fn(&T) -> T) -> Result<(), Self::Error>;

    /// Register a binary arithmetic metamethod where both operands are `T`.
    fn meta_arith_self(
        &mut self,
        op: &'static str,
        f: fn(T, T) -> T,
    ) -> Result<(), Self::Error>;

    /// Register a binary arithmetic metamethod where the rhs is a primitive.
    /// The backend handles commutativity (tries `T op rhs` then `rhs op T`).
    fn meta_arith_scalar(
        &mut self,
        op: &'static str,
        f: fn(T, &[ScriptValue]) -> Result<T, ScriptConvertError>,
    ) -> Result<(), Self::Error>;
}

// ---------------------------------------------------------------------------
// Free function binding
// ---------------------------------------------------------------------------

/// A free function that can register itself into a scripting runtime via
/// a [`FnBinder`].
///
/// Implemented on the hidden type emitted by `#[script]` on a free function.
pub trait ScriptBindFn {
    /// Register the function.
    fn bind<B: FnBinder>(binder: &mut B) -> Result<(), B::Error>;
}

/// Backend-provided registrar for free functions in a module.
pub trait FnBinder: Sized {
    /// The error type returned during registration.
    type Error: std::error::Error;

    /// Register a free function. The wrapper converts arguments from
    /// `ScriptValue` and returns the result as `ScriptValue` — the macro
    /// generates the conversion code with concrete types.
    fn function(
        &mut self,
        name: &'static str,
        f: fn(&[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>,
    ) -> Result<(), Self::Error>;
}

/// Identifies a type for [`TypeBinder`] dispatch.
#[derive(Debug, Clone, Copy)]
pub struct BindTarget<'a> {
    pub type_id: TypeId<'a>,
    pub name: &'a str,
}
