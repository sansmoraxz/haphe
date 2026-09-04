//! Compile-time evaluation matrix for [`HapheType`].
//!
//! These tests double as a regression guard for the const patterns the derive
//! macros rely on: taking `&T::DESCRIPTOR` inside generic associated-const
//! initializers, slices of generic consts, and deep nesting must all evaluate
//! in `static` initializers.

use std::collections::{BTreeMap, HashMap};

use haphe_core::{HapheType, PrimitiveType, TypeDescriptor};

const I32: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::I32);
const U8: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::U8);
const F64: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::F64);

#[test]
fn primitives() {
    assert_eq!(<i32 as HapheType>::DESCRIPTOR, I32);
    assert_eq!(
        <bool as HapheType>::DESCRIPTOR,
        TypeDescriptor::Primitive(PrimitiveType::Bool)
    );
    assert_eq!(
        <char as HapheType>::DESCRIPTOR,
        TypeDescriptor::Primitive(PrimitiveType::Char)
    );
    assert_eq!(
        <usize as HapheType>::DESCRIPTOR,
        TypeDescriptor::Primitive(PrimitiveType::U64)
    );
    assert_eq!(
        <isize as HapheType>::DESCRIPTOR,
        TypeDescriptor::Primitive(PrimitiveType::I64)
    );
}

#[test]
fn strings_and_unit() {
    assert_eq!(<String as HapheType>::DESCRIPTOR, TypeDescriptor::String);
    assert_eq!(<&str as HapheType>::DESCRIPTOR, TypeDescriptor::String);
    assert_eq!(<() as HapheType>::DESCRIPTOR, TypeDescriptor::Unit);
}

#[test]
fn containers() {
    assert_eq!(
        <Option<i32> as HapheType>::DESCRIPTOR,
        TypeDescriptor::Option(&I32)
    );
    assert_eq!(
        <Vec<i32> as HapheType>::DESCRIPTOR,
        TypeDescriptor::List(&I32)
    );
    assert_eq!(
        <&[i32] as HapheType>::DESCRIPTOR,
        TypeDescriptor::List(&I32)
    );
    assert_eq!(
        <[u8; 4] as HapheType>::DESCRIPTOR,
        TypeDescriptor::Array(&U8, 4)
    );
    assert_eq!(
        <HashMap<String, f64> as HapheType>::DESCRIPTOR,
        TypeDescriptor::Map(&TypeDescriptor::String, &F64),
    );
    assert_eq!(
        <BTreeMap<i32, i32> as HapheType>::DESCRIPTOR,
        TypeDescriptor::Map(&I32, &I32),
    );
    assert_eq!(
        <Result<i32, String> as HapheType>::DESCRIPTOR,
        TypeDescriptor::Result(&I32, &TypeDescriptor::String),
    );
    assert_eq!(<Box<str> as HapheType>::DESCRIPTOR, TypeDescriptor::String);
    assert_eq!(
        <&mut Vec<i32> as HapheType>::DESCRIPTOR,
        TypeDescriptor::List(&I32)
    );
}

#[test]
fn tuples() {
    assert_eq!(
        <(i32,) as HapheType>::DESCRIPTOR,
        TypeDescriptor::Tuple(&[I32])
    );
    assert_eq!(
        <(i32, String) as HapheType>::DESCRIPTOR,
        TypeDescriptor::Tuple(&[I32, TypeDescriptor::String]),
    );
    assert_eq!(
        <(u8, u8, u8, u8, u8, u8, u8, u8) as HapheType>::DESCRIPTOR,
        TypeDescriptor::Tuple(&[U8, U8, U8, U8, U8, U8, U8, U8]),
    );
}

#[test]
fn fn_pointers() {
    assert_eq!(
        <fn() -> i32 as HapheType>::DESCRIPTOR,
        TypeDescriptor::Callback {
            params: &[],
            return_type: &I32
        },
    );
    assert_eq!(
        <fn(i32, String) -> bool as HapheType>::DESCRIPTOR,
        TypeDescriptor::Callback {
            params: &[I32, TypeDescriptor::String],
            return_type: &TypeDescriptor::Primitive(PrimitiveType::Bool),
        },
    );
}

/// The critical spike: deep nesting evaluated in a `static` initializer —
/// exactly the shape derive-generated code produces.
static DEEP: TypeDescriptor<'static> =
    <Option<Vec<Result<(i32, String), [Vec<u8>; 4]>>> as HapheType>::DESCRIPTOR;

#[test]
fn deep_nesting_in_static() {
    let expected = TypeDescriptor::Option(&TypeDescriptor::List(&TypeDescriptor::Result(
        &TypeDescriptor::Tuple(&[I32, TypeDescriptor::String]),
        &TypeDescriptor::Array(&TypeDescriptor::List(&U8), 4),
    )));
    assert_eq!(DEEP, expected);
}

#[test]
fn smart_pointers_forward() {
    use std::borrow::Cow;
    use std::rc::Rc;
    use std::sync::Arc;
    assert_eq!(<Rc<i32> as HapheType>::DESCRIPTOR, I32);
    assert_eq!(<Arc<str> as HapheType>::DESCRIPTOR, TypeDescriptor::String);
    assert_eq!(
        <Cow<'_, str> as HapheType>::DESCRIPTOR,
        TypeDescriptor::String
    );
    assert_eq!(
        <Cow<'_, [u8]> as HapheType>::DESCRIPTOR,
        TypeDescriptor::List(&U8),
    );
}

#[test]
fn extended_arities() {
    assert_eq!(
        <(u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8) as HapheType>::DESCRIPTOR,
        TypeDescriptor::Tuple(&[U8; 12]),
    );
    let desc = <fn(u8, u8, u8, u8, u8, u8, u8, u8) -> u8 as HapheType>::DESCRIPTOR;
    assert_eq!(
        desc,
        TypeDescriptor::Callback {
            params: &[U8; 8],
            return_type: &U8
        }
    );
}
