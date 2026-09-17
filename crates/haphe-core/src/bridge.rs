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

use crate::types::{TypeDescriptor, TypeId};

// ---------------------------------------------------------------------------
// Value conversion
// ---------------------------------------------------------------------------

/// Type-erased wrapper for user-defined types flowing through [`ScriptValue`].
///
/// Uses `Arc` internally so that `ScriptValue` remains `Clone`. Values
/// constructed through [`new_typed`](Self::new_typed) carry their haphe
/// [`TypeId`] so dynamic dispatch can rank them exactly; untagged values
/// rank as merely coercible and resolve by try-call.
#[derive(Clone)]
pub struct OpaqueUserData {
    inner: Arc<dyn std::any::Any + Send + Sync>,
    type_tag: Option<TypeId<'static>>,
}

impl OpaqueUserData {
    pub fn new<T: Send + Sync + 'static>(value: T) -> Self {
        Self {
            inner: Arc::new(value),
            type_tag: None,
        }
    }

    /// Wraps a described type, capturing its [`TypeId`] for dynamic
    /// dispatch.
    pub fn new_typed<T: crate::script::ScriptType + Send + Sync + 'static>(value: T) -> Self {
        Self {
            inner: Arc::new(value),
            type_tag: Some(<T as crate::script::ScriptType>::ID),
        }
    }

    /// The described type's id, when constructed via
    /// [`new_typed`](Self::new_typed).
    pub fn type_tag(&self) -> Option<TypeId<'static>> {
        self.type_tag
    }

    pub fn downcast_ref<T: 'static>(&self) -> Option<&T> {
        self.inner.downcast_ref()
    }

    pub fn downcast_clone<T: Clone + 'static>(&self) -> Option<T> {
        self.inner.downcast_ref::<T>().cloned()
    }
}

impl std::fmt::Debug for OpaqueUserData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("UserData")
            .field(&self.inner.type_id())
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
    /// A unit-enum value, identified by its declared case name.
    ///
    /// The case always carries the *declared* exposed spelling
    /// (identifier-shaped, Rust conventions: ASCII letters, digits, `_`) and
    /// is matched exactly. A backend whose native convention spells cases
    /// differently translates to and from the declared name at its own
    /// boundary, with a comparison policy of its choosing.
    Enum {
        /// The declared case name.
        case: String,
        /// The numeric discriminant, when the enum has a numeric script
        /// representation ([`repr`](crate::EnumDescriptor::repr)); `None`
        /// for string-represented enums. The case NAME remains the
        /// canonical identity; the discriminant is carried data backends
        /// may render natively.
        discriminant: Option<i64>,
    },
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
            Self::Enum { .. } => "enum",
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
        Self::Map(
            m.into_iter()
                .map(|(k, v)| (k, ScriptValue::from(v)))
                .collect(),
        )
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

// Tuples cross the bridge as fixed-length lists, matching their descriptor
// shape (`TypeDescriptor::Tuple`).
macro_rules! impl_tuple_bridge {
    ($( ($($t:ident $idx:tt),+) ),+ $(,)?) => {$(
        impl<$($t),+> From<($($t,)+)> for ScriptValue
        where
            $(ScriptValue: From<$t>,)+
        {
            fn from(value: ($($t,)+)) -> Self {
                Self::List(vec![$(ScriptValue::from(value.$idx)),+])
            }
        }

        impl<$($t: FromScript),+> FromScript for ($($t,)+) {
            fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
                const ARITY: usize = 0 $(+ impl_tuple_bridge!(@one $t))+;
                match v {
                    ScriptValue::List(items) => {
                        if items.len() != ARITY {
                            return Err(ScriptConvertError {
                                expected: "tuple",
                                got: "list of mismatched length",
                            });
                        }
                        let mut items = items.into_iter();
                        Ok(($($t::from_script(items.next().expect("length checked"))?,)+))
                    }
                    other => Err(ScriptConvertError {
                        expected: "tuple",
                        got: other.variant_name(),
                    }),
                }
            }
        }
    )+};
    (@one $t:ident) => { 1 };
}

impl_tuple_bridge! {
    (A 0),
    (A 0, B 1),
    (A 0, B 1, C 2),
    (A 0, B 1, C 2, D 3),
    (A 0, B 1, C 2, D 3, E 4),
    (A 0, B 1, C 2, D 3, E 4, F 5),
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6),
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7),
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8),
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9),
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9, K 10),
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9, K 10, L 11),
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

    /// Register a non-mutating method (`&self` or consuming `self`). The
    /// wrapper converts arguments from `ScriptValue` and returns the result
    /// as `ScriptValue` — the macro generates the conversion code with
    /// concrete types. The receiver arrives as a [`ScriptCow`]: pass
    /// `Borrowed` while holding the runtime's guard for zero-clone dispatch,
    /// or `Owned` with a value acquired by the backend's own policy (a
    /// consuming method clones a borrowed carrier at the boundary).
    fn method(
        &mut self,
        name: &'static str,
        f: for<'a> fn(ScriptCow<'a, T>, &[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>,
    ) -> Result<(), Self::Error>;

    /// Register a `&mut self` method.
    fn method_mut(
        &mut self,
        name: &'static str,
        f: fn(&mut T, &[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>,
    ) -> Result<(), Self::Error>;

    /// Register a non-mutating `async` method (`&self` or consuming
    /// `self`).
    ///
    /// The receiver arrives as a [`ScriptCow`] and the returned future may
    /// borrow it: a backend that can hold its runtime's guard across
    /// `await`s passes `Borrowed` (zero clones); one that cannot passes
    /// `Owned`. Only reachable when the backend's capabilities declare
    /// async support — others reject the registration with a descriptive
    /// error.
    fn method_async(
        &mut self,
        name: &'static str,
        f: for<'a> fn(ScriptCow<'a, T>, &'a [ScriptValue]) -> ScriptCallFuture<'a>,
    ) -> Result<(), Self::Error>;

    /// Register a `&mut self` `async` method.
    ///
    /// The future borrows the receiver mutably for its whole run, so
    /// mutation writes back in place. A backend that cannot hold a mutable
    /// guard across `await`s rejects the registration with a descriptive
    /// error — it must never substitute an acquired copy, whose mutations
    /// would be silently lost.
    fn method_async_mut(
        &mut self,
        name: &'static str,
        f: for<'a> fn(&'a mut T, &'a [ScriptValue]) -> ScriptCallFuture<'a>,
    ) -> Result<(), Self::Error>;

    /// Register one declared instantiation of a STATICALLY dispatched
    /// generic method (`&self`, consuming `self`, or receiver-less):
    /// `type_args` carries that instantiation's concrete type arguments (in
    /// declaration order, matching an entry of the descriptor's
    /// [`instantiations`](crate::FunctionDescriptor::instantiations)) and
    /// `f` is the corresponding monomorphized wrapper. Backends key the
    /// registration on `(name, type_args)` — typically a mangled per-monomorph
    /// method name, mirroring their static generic free-function scheme.
    ///
    /// The default delegates to [`method`](Self::method) — safe because the
    /// capability check rejects generic registrations before binding on
    /// backends without [`generics`](crate::BackendCapabilities::generics);
    /// a backend that declares the capability MUST override, or same-named
    /// monomorphs would collide.
    fn method_generic(
        &mut self,
        name: &'static str,
        type_args: &'static [crate::types::TypeDescriptor<'static>],
        f: for<'a> fn(ScriptCow<'a, T>, &[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>,
    ) -> Result<(), Self::Error> {
        let _ = type_args;
        self.method(name, f)
    }

    /// `&mut self` sibling of [`method_generic`](Self::method_generic).
    fn method_generic_mut(
        &mut self,
        name: &'static str,
        type_args: &'static [crate::types::TypeDescriptor<'static>],
        f: fn(&mut T, &[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>,
    ) -> Result<(), Self::Error> {
        let _ = type_args;
        self.method_mut(name, f)
    }

    /// Async sibling of [`method_generic`](Self::method_generic);
    /// async-capability gating applies, like
    /// [`method_async`](Self::method_async).
    fn method_generic_async(
        &mut self,
        name: &'static str,
        type_args: &'static [crate::types::TypeDescriptor<'static>],
        f: for<'a> fn(ScriptCow<'a, T>, &'a [ScriptValue]) -> ScriptCallFuture<'a>,
    ) -> Result<(), Self::Error> {
        let _ = type_args;
        self.method_async(name, f)
    }

    /// Async `&mut self` sibling of
    /// [`method_generic`](Self::method_generic).
    fn method_generic_async_mut(
        &mut self,
        name: &'static str,
        type_args: &'static [crate::types::TypeDescriptor<'static>],
        f: for<'a> fn(&'a mut T, &'a [ScriptValue]) -> ScriptCallFuture<'a>,
    ) -> Result<(), Self::Error> {
        let _ = type_args;
        self.method_async_mut(name, f)
    }

    /// Register one candidate of a `dyn`-dispatched generic method (`&self`,
    /// consuming `self`, or receiver-less).
    ///
    /// Called once per declared instantiation, like
    /// [`method`](Self::method); the whole descriptor is provided because
    /// dynamic resolution ranks candidates against the declared parameter
    /// types (see
    /// [`resolve_dyn_candidate`](crate::dispatch::resolve_dyn_candidate)).
    /// The default delegates to `method` — safe because the capability check
    /// rejects `dyn` methods before binding on backends without
    /// [`dyn_generics`](crate::BackendCapabilities::dyn_generics).
    fn method_dyn(
        &mut self,
        descriptor: &'static crate::function::FunctionDescriptor<'static>,
        type_args: &'static [crate::types::TypeDescriptor<'static>],
        f: for<'a> fn(ScriptCow<'a, T>, &[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>,
    ) -> Result<(), Self::Error> {
        let _ = type_args;
        self.method(descriptor.name, f)
    }

    /// `&mut self` sibling of [`method_dyn`](Self::method_dyn).
    fn method_dyn_mut(
        &mut self,
        descriptor: &'static crate::function::FunctionDescriptor<'static>,
        type_args: &'static [crate::types::TypeDescriptor<'static>],
        f: fn(&mut T, &[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>,
    ) -> Result<(), Self::Error> {
        let _ = type_args;
        self.method_mut(descriptor.name, f)
    }

    /// Async sibling of [`method_dyn`](Self::method_dyn).
    fn method_dyn_async(
        &mut self,
        descriptor: &'static crate::function::FunctionDescriptor<'static>,
        type_args: &'static [crate::types::TypeDescriptor<'static>],
        f: for<'a> fn(ScriptCow<'a, T>, &'a [ScriptValue]) -> ScriptCallFuture<'a>,
    ) -> Result<(), Self::Error> {
        let _ = type_args;
        self.method_async(descriptor.name, f)
    }

    /// Async `&mut self` sibling of [`method_dyn`](Self::method_dyn).
    fn method_dyn_async_mut(
        &mut self,
        descriptor: &'static crate::function::FunctionDescriptor<'static>,
        type_args: &'static [crate::types::TypeDescriptor<'static>],
        f: for<'a> fn(&'a mut T, &'a [ScriptValue]) -> ScriptCallFuture<'a>,
    ) -> Result<(), Self::Error> {
        let _ = type_args;
        self.method_async_mut(descriptor.name, f)
    }

    /// Register an `async` constructor. The returned future may borrow the
    /// argument slice; async-capability gating applies, like
    /// [`method_async`](Self::method_async).
    fn constructor_async(
        &mut self,
        name: &'static str,
        f: for<'a> fn(&'a [ScriptValue]) -> ScriptCtorFuture<'a, T>,
    ) -> Result<(), Self::Error>;

    /// Register a computed-property getter, from `#[script(getter)]`.
    /// Conversion is infallible on the way out; the setter side converts
    /// fallibly.
    fn property_get(
        &mut self,
        name: &'static str,
        f: fn(&T) -> ScriptValue,
    ) -> Result<(), Self::Error>;

    /// Register a computed-property setter, from `#[script(setter)]`.
    fn property_set(
        &mut self,
        name: &'static str,
        f: fn(&mut T, ScriptValue) -> Result<(), ScriptConvertError>,
    ) -> Result<(), Self::Error>;

    /// Register an `async` computed-property getter. The receiver arrives
    /// as a [`ScriptCow`] the future may borrow (see
    /// [`method_async`](Self::method_async)); async-capability gating
    /// applies.
    fn property_get_async(
        &mut self,
        name: &'static str,
        f: for<'a> fn(ScriptCow<'a, T>) -> ScriptCallFuture<'a>,
    ) -> Result<(), Self::Error>;

    /// Register an `async` computed-property setter. The future borrows the
    /// receiver mutably for its whole run, so mutation writes back in place
    /// (see [`method_async_mut`](Self::method_async_mut)).
    fn property_set_async(
        &mut self,
        name: &'static str,
        f: for<'a> fn(&'a mut T, ScriptValue) -> ScriptCallFuture<'a>,
    ) -> Result<(), Self::Error>;

    /// Register a constructor (no receiver, returns `T`).
    fn constructor(
        &mut self,
        name: &'static str,
        f: fn(&[ScriptValue]) -> Result<T, ScriptConvertError>,
    ) -> Result<(), Self::Error>;

    /// Register a `Display` / `__tostring` metamethod.
    fn meta_tostring(&mut self, f: fn(&T) -> String) -> Result<(), Self::Error>;

    /// Register a `ToString`-derived concatenation metamethod.
    ///
    /// Concatenation is string-producing: the backend formats the value
    /// operand(s) with `f` and accepts only string-like counterparts
    /// (strings and numbers), erroring on anything else.
    fn meta_concat(&mut self, f: fn(&T) -> String) -> Result<(), Self::Error>;

    /// Register a hash accessor, from `std::hash::Hash` (a stable `u64`
    /// digest via the standard hasher). Backends surface it in their native
    /// shape — a named function where the runtime has no hashing protocol.
    fn meta_hash(&mut self, f: fn(&T) -> u64) -> Result<(), Self::Error>;

    /// Register a debug-formatting accessor, from `std::fmt::Debug`.
    /// Distinct from [`meta_tostring`](Self::meta_tostring): both may be
    /// registered; backends decide how each surfaces.
    fn meta_debug(&mut self, f: fn(&T) -> String) -> Result<(), Self::Error>;

    /// Register a `PartialEq` / `__eq` metamethod.
    fn meta_eq(&mut self, f: fn(&T, &T) -> bool) -> Result<(), Self::Error>;

    /// Register a `__lt` metamethod.
    fn meta_lt(&mut self, f: fn(&T, &T) -> bool) -> Result<(), Self::Error>;

    /// Register a `__le` metamethod.
    fn meta_le(&mut self, f: fn(&T, &T) -> bool) -> Result<(), Self::Error>;

    /// Register a unary negation metamethod.
    fn meta_unm(&mut self, f: fn(&T) -> T) -> Result<(), Self::Error>;

    /// Register a bitwise-not metamethod, from `std::ops::Not`.
    ///
    /// A backend whose runtime has no bitwise-operator construct (or not in
    /// the configured version) rejects the registration with a descriptive
    /// error rather than dropping it.
    fn meta_bnot(&mut self, f: fn(&T) -> T) -> Result<(), Self::Error>;

    /// Register a binary operator metamethod where both operands are `T`.
    ///
    /// `op` is the Rust trait method name: `add`, `sub`, `mul`, `div`, `rem`
    /// for arithmetic, `idiv` for floor division ([`crate::ops::IDiv`]),
    /// `mod` for floor modulo ([`crate::ops::Mod`]), `pow` for
    /// exponentiation ([`crate::ops::Pow`]), and `bitand`, `bitor`,
    /// `bitxor`, `shl`, `shr` for the bitwise family. A backend whose runtime lacks a construct for an op
    /// (or not in the configured version) rejects the registration with a
    /// descriptive error rather than dropping it.
    fn meta_arith_self(&mut self, op: &'static str, f: fn(T, T) -> T) -> Result<(), Self::Error>;

    /// Register a binary operator metamethod where the rhs is a primitive.
    /// The backend handles commutativity (tries `T op rhs` then `rhs op T`).
    ///
    /// `rhs` describes the scalar operand's declared type so backends with
    /// dynamic dispatch can rank overloads (e.g. prefer an exact type match
    /// over the first lossy conversion that succeeds); the ranking policy is
    /// the backend's own.
    fn meta_arith_scalar(
        &mut self,
        op: &'static str,
        rhs: &'static crate::types::TypeDescriptor<'static>,
        f: fn(T, &[ScriptValue]) -> Result<T, ScriptConvertError>,
    ) -> Result<(), Self::Error>;

    /// Register an iteration metamethod, from `IntoIterator`.
    ///
    /// `f` consumes an owned value and yields an owned lazy iterator over its
    /// contents; the backend holds the iterator and drives its runtime's
    /// native iteration protocol with it. How the backend obtains the owned
    /// value (cloning, taking) is its own policy — the bridge enforces no
    /// acquisition strategy.
    fn meta_iter(&mut self, f: fn(T) -> ScriptIter) -> Result<(), Self::Error>;

    /// Register a length metamethod, derived from the iterator's
    /// [`size_hint`](Iterator::size_hint) lower bound: exact for standard
    /// containers (their owned iterators are exact-size), a lower bound
    /// otherwise. Consuming, like [`meta_iter`](Self::meta_iter).
    ///
    /// Computing the length constructs one iterator (plus the backend's
    /// value-acquisition cost).
    fn meta_len(&mut self, f: fn(T) -> usize) -> Result<(), Self::Error>;

    /// Register a call metamethod, from [`crate::ops::Call`].
    ///
    /// `f` converts the call arguments to the declared tuple, invokes
    /// through a shared reference, and converts the result back. A backend
    /// whose runtime has no call construct rejects the registration with a
    /// descriptive error rather than dropping it.
    fn meta_call(
        &mut self,
        f: fn(&T, &[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>,
    ) -> Result<(), Self::Error>;

    /// Register an async call metamethod, from [`crate::ops::AsyncCall`].
    ///
    /// The receiver arrives as a [`ScriptCow`] like
    /// [`method_async`](Self::method_async). Only reachable when the
    /// backend's capabilities declare async support — others reject the
    /// registration with a descriptive error.
    fn meta_call_async(
        &mut self,
        f: for<'a> fn(ScriptCow<'a, T>, &'a [ScriptValue]) -> ScriptCallFuture<'a>,
    ) -> Result<(), Self::Error>;

    /// Register an indexed-read metamethod, from `std::ops::Index`.
    ///
    /// `f` converts the single key argument to the declared index type,
    /// indexes, and returns the output clone-at-boundary. The key converts
    /// verbatim — no base adjustment between the runtime's and Rust's
    /// indexing conventions. An out-of-bounds Rust panic surfaces as the
    /// backend's runtime error.
    fn meta_index(
        &mut self,
        f: fn(&T, &[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>,
    ) -> Result<(), Self::Error>;

    /// Register an indexed-write metamethod, from `std::ops::IndexMut`.
    ///
    /// `f` receives `[key, value]`, converts both to the declared index and
    /// output types, and assigns in place. Same verbatim-key and panic
    /// semantics as [`meta_index`](Self::meta_index).
    fn meta_newindex(
        &mut self,
        f: fn(&mut T, &[ScriptValue]) -> Result<(), ScriptConvertError>,
    ) -> Result<(), Self::Error>;
}

/// Clone-on-write receiver carrier for bridged method and call dispatch.
///
/// The backend picks the variant per its own model: one that can hold its
/// runtime's userdata guard across the invocation (or across `await`s)
/// passes [`Borrowed`](ScriptCow::Borrowed) — zero clones, and for `&mut`
/// channels true in-place mutation; one that cannot passes
/// [`Owned`](ScriptCow::Owned) with a value it acquired by its own policy.
/// Generated wrappers consume the carrier: shared-receiver methods go
/// through [`Deref`](core::ops::Deref), consuming ones take the owned value
/// (cloning a borrowed carrier at the boundary).
///
/// Bridge plumbing, not a bridged value type: it never appears in a
/// descriptor or a script-visible signature, so it deliberately implements
/// none of the `Script*` description traits.
pub enum ScriptCow<'a, T> {
    /// A receiver borrowed from the runtime for the duration of the call.
    Borrowed(&'a T),
    /// A receiver the backend acquired and handed over.
    Owned(T),
}

impl<T> core::ops::Deref for ScriptCow<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        match self {
            Self::Borrowed(t) => t,
            Self::Owned(t) => t,
        }
    }
}

impl<T: Clone> ScriptCow<'_, T> {
    /// Extracts an owned value, cloning a borrowed carrier.
    pub fn into_owned(self) -> T {
        match self {
            Self::Borrowed(t) => t.clone(),
            Self::Owned(t) => t,
        }
    }
}

/// Boxed future produced by async bridged dispatch
/// ([`TypeBinder::method_async`] and friends). May borrow the receiver
/// carrier and the argument slice for `'a` — the backend drives it while
/// holding whatever guard backs a [`ScriptCow::Borrowed`] receiver.
///
/// Intentionally not `Send`; a backend whose threading model demands `Send`
/// handles that at its own boundary.
pub type ScriptCallFuture<'a> = ::core::pin::Pin<
    Box<dyn ::core::future::Future<Output = Result<ScriptValue, ScriptConvertError>> + 'a>,
>;

/// Boxed future produced by an async constructor
/// ([`TypeBinder::constructor_async`]): resolves to the constructed value.
/// Not `Send`, like [`ScriptCallFuture`].
pub type ScriptCtorFuture<'a, T> =
    ::core::pin::Pin<Box<dyn ::core::future::Future<Output = Result<T, ScriptConvertError>> + 'a>>;

/// Owned iterator over a value's contents.
///
/// Produced from `IntoIterator` on an owned value, so it borrows nothing and
/// a backend can hold it across its runtime's iteration steps. Items are
/// plain values — any pairing or enumeration a runtime's iteration protocol
/// needs is the backend's job.
///
/// Intentionally not `Send`; a backend whose threading model demands `Send`
/// handles that at its own boundary.
pub struct ScriptIter(Box<dyn Iterator<Item = ScriptValue>>);

impl ScriptIter {
    /// Wraps an owned iterator whose items are already converted.
    pub fn new(inner: impl Iterator<Item = ScriptValue> + 'static) -> Self {
        Self(Box::new(inner))
    }
}

impl Iterator for ScriptIter {
    type Item = ScriptValue;

    fn next(&mut self) -> Option<ScriptValue> {
        self.0.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl std::fmt::Debug for ScriptIter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptIter").finish_non_exhaustive()
    }
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
    ///
    /// A generic function registers once per declared instantiation:
    /// `type_args` carries that instantiation's concrete type arguments (in
    /// declaration order, matching an entry of the descriptor's
    /// [`instantiations`](crate::FunctionDescriptor::instantiations)) and `f`
    /// is the corresponding monomorphized wrapper. `type_args` is empty for
    /// non-generic functions.
    fn function(
        &mut self,
        name: &'static str,
        type_args: &'static [crate::types::TypeDescriptor<'static>],
        f: fn(&[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>,
    ) -> Result<(), Self::Error>;

    /// Register an `async` free function. The returned future may borrow the
    /// argument slice for `'a`. Only reachable when the backend's
    /// capabilities declare async support — others reject the registration
    /// with a descriptive error rather than dropping it.
    fn function_async(
        &mut self,
        name: &'static str,
        type_args: &'static [crate::types::TypeDescriptor<'static>],
        f: for<'a> fn(&'a [ScriptValue]) -> ScriptCallFuture<'a>,
    ) -> Result<(), Self::Error>;

    /// Register one candidate of a `dyn`-dispatched generic function.
    ///
    /// Called once per declared instantiation, like
    /// [`function`](Self::function); the whole descriptor is provided
    /// because dynamic resolution ranks candidates against the declared
    /// parameter types (see
    /// [`resolve_dyn_candidate`](crate::dispatch::resolve_dyn_candidate)).
    /// The default delegates to `function` — safe because the capability
    /// check rejects `dyn` functions before binding on backends without
    /// [`dyn_generics`](crate::BackendCapabilities::dyn_generics), and a
    /// bypassing bind still hits the backend's loud static-generic path.
    fn function_dyn(
        &mut self,
        descriptor: &'static crate::function::FunctionDescriptor<'static>,
        type_args: &'static [crate::types::TypeDescriptor<'static>],
        f: fn(&[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>,
    ) -> Result<(), Self::Error> {
        self.function(descriptor.name, type_args, f)
    }

    /// Async sibling of [`function_dyn`](Self::function_dyn).
    fn function_dyn_async(
        &mut self,
        descriptor: &'static crate::function::FunctionDescriptor<'static>,
        type_args: &'static [crate::types::TypeDescriptor<'static>],
        f: for<'a> fn(&'a [ScriptValue]) -> ScriptCallFuture<'a>,
    ) -> Result<(), Self::Error> {
        self.function_async(descriptor.name, type_args, f)
    }
}

// ---------------------------------------------------------------------------
// Compile-time bridgeability dispatch (macro plumbing)
// ---------------------------------------------------------------------------

/// Carrier for compile-time bridgeability dispatch (autoref
/// specialization): macro-generated registration code probes whether a
/// signature's types implement the bridge traits and registers only when
/// they do, falling back to a no-op otherwise.
#[doc(hidden)]
pub struct BridgeProbe<S: ?Sized>(pub core::marker::PhantomData<S>);

/// The no-op fallback arm of bridgeability dispatch. Macro-generated `__Go`
/// traits on `&BridgeProbe<S>` take precedence when the signature's bridge
/// bounds hold.
#[doc(hidden)]
pub trait SkipBind<T> {
    fn __haphe_bind<B: TypeBinder<T>>(&self, _b: &mut B) -> Result<(), B::Error> {
        Ok(())
    }
}

impl<S: ?Sized, T> SkipBind<T> for BridgeProbe<S> {}

/// The no-op fallback arm of bridgeability dispatch for free functions
/// (mirrors [`SkipBind`] against [`FnBinder`]).
#[doc(hidden)]
pub trait SkipBindFn {
    fn __haphe_bind_fn<B: FnBinder>(&self, _b: &mut B) -> Result<(), B::Error> {
        Ok(())
    }
}

impl<S: ?Sized> SkipBindFn for BridgeProbe<S> {}

/// Identifies a type for [`TypeBinder`] dispatch.
#[derive(Debug, Clone, Copy)]
pub struct BindTarget<'a> {
    pub type_id: TypeId<'a>,
    pub name: &'a str,
}

// ---------------------------------------------------------------------------
// Foreign function calling
// ---------------------------------------------------------------------------

/// Backend-provided dispatcher for host-supplied functions.
///
/// One caller backs one foreign interface (one instantiation of it, for a
/// generic interface): it resolves `function` to the host's implementation,
/// invokes it with the given arguments, and converts the result back to a
/// [`ScriptValue`].
///
/// `type_args` carries the concrete type arguments of a function-level
/// generic call, in declaration order — empty for non-generic functions.
/// Backends that monomorphize select the implementation by comparing them
/// against the interface's recorded
/// [`MethodInstantiation`](crate::MethodInstantiation)s; dynamic backends may
/// ignore them.
pub trait ForeignCaller {
    /// Invokes the named host function synchronously.
    fn call(
        &self,
        function: &'static str,
        type_args: &[TypeDescriptor<'static>],
        args: &[ScriptValue],
    ) -> Result<ScriptValue, ForeignError>;

    /// Invokes the named host function asynchronously.
    ///
    /// The default implementation fails with
    /// [`ForeignErrorKind::AsyncUnsupported`]; backends whose capabilities
    /// declare `async_fns` must override it. Registries with async foreign
    /// functions are rejected up front by the capability check when the
    /// backend does not support async.
    fn call_async<'a>(
        &'a self,
        function: &'static str,
        type_args: &'a [TypeDescriptor<'static>],
        args: &'a [ScriptValue],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ScriptValue, ForeignError>> + 'a>>
    {
        let _ = (type_args, args);
        Box::pin(std::future::ready(Err(ForeignError {
            function,
            kind: ForeignErrorKind::AsyncUnsupported,
        })))
    }
}

/// Error raised while dispatching a foreign function call.
#[derive(Debug)]
pub struct ForeignError {
    /// The foreign function that was being called.
    pub function: &'static str,
    /// What went wrong.
    pub kind: ForeignErrorKind,
}

/// The failure modes of a foreign function call.
#[derive(Debug)]
#[non_exhaustive]
pub enum ForeignErrorKind {
    /// The host raised an error while running the function.
    Call(Box<dyn std::error::Error + Send + Sync>),
    /// The host's return value did not convert to the declared Rust type.
    Convert(ScriptConvertError),
    /// The caller does not support asynchronous dispatch.
    AsyncUnsupported,
}

impl std::fmt::Display for ForeignError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            ForeignErrorKind::Call(e) => {
                write!(f, "foreign function `{}` failed: {e}", self.function)
            }
            ForeignErrorKind::Convert(e) => write!(
                f,
                "foreign function `{}` returned an unexpected value: {e}",
                self.function
            ),
            ForeignErrorKind::AsyncUnsupported => write!(
                f,
                "foreign function `{}` is async but the caller does not support async dispatch",
                self.function
            ),
        }
    }
}

impl std::error::Error for ForeignError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            ForeignErrorKind::Call(e) => Some(e.as_ref()),
            ForeignErrorKind::Convert(e) => Some(e),
            ForeignErrorKind::AsyncUnsupported => None,
        }
    }
}

/// A foreign-interface handle constructible from a [`ForeignCaller`].
///
/// Implemented by the handle type generated for a foreign trait; backends
/// use it to hand out trait implementations backed by their runtime.
pub trait ForeignHandle: Sized {
    /// Wraps a backend caller into the handle.
    fn from_caller(caller: Box<dyn ForeignCaller>) -> Self;
}
