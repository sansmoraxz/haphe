use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::sync::Arc;

use crate::types::{PrimitiveType, TypeDescriptor};

/// A type with a compile-time [`TypeDescriptor`].
///
/// Implemented for primitives and common standard-library types: `String`,
/// `Option<T>`, `Vec<T>`, slices, arrays, tuples, `HashMap`/`BTreeMap`,
/// `Result<T, E>`, references, `Box`/`Rc`/`Arc`/`Cow`, and `fn` pointers with by-value
/// parameters (`fn(&str) -> _` is higher-ranked and cannot be covered — take
/// owned values in callback signatures). User types get an implementation
/// through `#[derive(Script)]`.
///
/// Descriptors resolve through the trait system, so type aliases and
/// re-exports are handled by the compiler.
///
/// # Example
///
/// ```
/// use haphe_core::{HapheType, TypeDescriptor, PrimitiveType};
///
/// static DESC: TypeDescriptor<'static> = <Option<Vec<i32>> as HapheType>::DESCRIPTOR;
/// assert_eq!(
///     DESC,
///     TypeDescriptor::Option(&TypeDescriptor::List(&TypeDescriptor::Primitive(
///         PrimitiveType::I32
///     ))),
/// );
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be exposed to scripting runtimes",
    label = "no `HapheType` descriptor for `{Self}`",
    note = "if `{Self}` is your type, add `#[derive(Script)]` to it",
    note = "if `{Self}` is a third-party type, wrap it in a newtype and derive `Script` on that"
)]
pub trait HapheType {
    /// The language-agnostic description of this type.
    const DESCRIPTOR: TypeDescriptor<'static>;
}

macro_rules! impl_primitive {
    ($($ty:ty => $variant:ident),* $(,)?) => {
        $(
            impl HapheType for $ty {
                const DESCRIPTOR: TypeDescriptor<'static> =
                    TypeDescriptor::Primitive(PrimitiveType::$variant);
            }
        )*
    };
}

impl_primitive! {
    bool => Bool,
    i8 => I8,
    i16 => I16,
    i32 => I32,
    i64 => I64,
    i128 => I128,
    u8 => U8,
    u16 => U16,
    u32 => U32,
    u64 => U64,
    u128 => U128,
    f32 => F32,
    f64 => F64,
    char => Char,
}

/// Described as [`PrimitiveType::U64`]; scripting languages have no
/// pointer-width integer type.
impl HapheType for usize {
    const DESCRIPTOR: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::U64);
}

/// Described as [`PrimitiveType::I64`]; scripting languages have no
/// pointer-width integer type.
impl HapheType for isize {
    const DESCRIPTOR: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::I64);
}

impl HapheType for String {
    const DESCRIPTOR: TypeDescriptor<'static> = TypeDescriptor::String;
}

impl HapheType for str {
    const DESCRIPTOR: TypeDescriptor<'static> = TypeDescriptor::String;
}

impl HapheType for () {
    const DESCRIPTOR: TypeDescriptor<'static> = TypeDescriptor::Unit;
}

impl<T: HapheType> HapheType for Option<T> {
    const DESCRIPTOR: TypeDescriptor<'static> = TypeDescriptor::Option(&T::DESCRIPTOR);
}

impl<T: HapheType> HapheType for Vec<T> {
    const DESCRIPTOR: TypeDescriptor<'static> = TypeDescriptor::List(&T::DESCRIPTOR);
}

impl<T: HapheType> HapheType for [T] {
    const DESCRIPTOR: TypeDescriptor<'static> = TypeDescriptor::List(&T::DESCRIPTOR);
}

impl<T: HapheType, const N: usize> HapheType for [T; N] {
    const DESCRIPTOR: TypeDescriptor<'static> = TypeDescriptor::Array(&T::DESCRIPTOR, N);
}

impl<K: HapheType, V: HapheType> HapheType for HashMap<K, V> {
    const DESCRIPTOR: TypeDescriptor<'static> = TypeDescriptor::Map(&K::DESCRIPTOR, &V::DESCRIPTOR);
}

impl<K: HapheType, V: HapheType> HapheType for BTreeMap<K, V> {
    const DESCRIPTOR: TypeDescriptor<'static> = TypeDescriptor::Map(&K::DESCRIPTOR, &V::DESCRIPTOR);
}

impl<T: HapheType, E: HapheType> HapheType for Result<T, E> {
    const DESCRIPTOR: TypeDescriptor<'static> =
        TypeDescriptor::Result(&T::DESCRIPTOR, &E::DESCRIPTOR);
}

impl<T: HapheType + ?Sized> HapheType for &T {
    const DESCRIPTOR: TypeDescriptor<'static> = T::DESCRIPTOR;
}

impl<T: HapheType + ?Sized> HapheType for &mut T {
    const DESCRIPTOR: TypeDescriptor<'static> = T::DESCRIPTOR;
}

impl<T: HapheType + ?Sized> HapheType for Box<T> {
    const DESCRIPTOR: TypeDescriptor<'static> = T::DESCRIPTOR;
}

impl<T: HapheType + ?Sized> HapheType for Rc<T> {
    const DESCRIPTOR: TypeDescriptor<'static> = T::DESCRIPTOR;
}

impl<T: HapheType + ?Sized> HapheType for Arc<T> {
    const DESCRIPTOR: TypeDescriptor<'static> = T::DESCRIPTOR;
}

impl<T: HapheType + ToOwned + ?Sized> HapheType for Cow<'_, T> {
    const DESCRIPTOR: TypeDescriptor<'static> = T::DESCRIPTOR;
}

macro_rules! impl_tuple {
    () => {};
    ($first:ident $(, $rest:ident)*) => {
        impl<$first: HapheType $(, $rest: HapheType)*> HapheType for ($first, $($rest,)*) {
            const DESCRIPTOR: TypeDescriptor<'static> =
                TypeDescriptor::Tuple(&[$first::DESCRIPTOR $(, $rest::DESCRIPTOR)*]);
        }
        impl_tuple!($($rest),*);
    };
}

impl_tuple!(A, B, C, D, E, F, G, H, I, J, K, L);

macro_rules! impl_fn {
    () => {
        impl<R: HapheType> HapheType for fn() -> R {
            const DESCRIPTOR: TypeDescriptor<'static> = TypeDescriptor::Callback {
                params: &[],
                return_type: &R::DESCRIPTOR,
            };
        }
    };
    ($first:ident $(, $rest:ident)*) => {
        impl<$first: HapheType $(, $rest: HapheType)*, R: HapheType> HapheType
            for fn($first $(, $rest)*) -> R
        {
            const DESCRIPTOR: TypeDescriptor<'static> = TypeDescriptor::Callback {
                params: &[$first::DESCRIPTOR $(, $rest::DESCRIPTOR)*],
                return_type: &R::DESCRIPTOR,
            };
        }
        impl_fn!($($rest),*);
    };
}

impl_fn!(A, B, C, D, E, F, G, H);
