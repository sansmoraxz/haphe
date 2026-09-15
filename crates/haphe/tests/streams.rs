//! Derive-side folding of first-class stream/future types.

#![cfg(all(feature = "macros", feature = "streams", feature = "futures"))]
#![allow(dead_code)]

use haphe::{Script, ScriptStruct, TypeDescriptor};

/// Concrete uses resolve through the `HapheType` trait chain.
#[derive(Script)]
struct Feed {
    events: haphe::Stream<i32>,
    done: haphe::Future<bool>,
}

#[test]
fn concrete_stream_fields_fold_via_trait() {
    let desc = <Feed as ScriptStruct>::DESCRIPTOR;
    assert!(matches!(
        *desc.fields[0].ty,
        TypeDescriptor::Stream(&TypeDescriptor::Primitive(haphe::PrimitiveType::I32))
    ));
    assert!(matches!(
        *desc.fields[1].ty,
        TypeDescriptor::Future(&TypeDescriptor::Primitive(haphe::PrimitiveType::Bool))
    ));
}

/// Generic positions fold syntactically, like `Vec<T>`.
#[derive(Script)]
struct Sub<T> {
    events: haphe::Stream<T>,
    next: haphe::Future<T>,
}

#[test]
fn generic_stream_fields_fold_to_generic_param() {
    let desc = <Sub<i32> as ScriptStruct>::DESCRIPTOR;
    assert_eq!(
        *desc.fields[0].ty,
        TypeDescriptor::Stream(&TypeDescriptor::GenericParam("T"))
    );
    assert_eq!(
        *desc.fields[1].ty,
        TypeDescriptor::Future(&TypeDescriptor::GenericParam("T"))
    );
}

use haphe::{HapheType, PrimitiveType};

const I32: TypeDescriptor = TypeDescriptor::Primitive(PrimitiveType::I32);

#[test]
fn stream_descriptor_shape() {
    assert_eq!(
        <haphe::Stream<i32> as HapheType>::DESCRIPTOR,
        TypeDescriptor::Stream(&I32)
    );
    assert_eq!(
        <haphe::Future<String> as HapheType>::DESCRIPTOR,
        TypeDescriptor::Future(&TypeDescriptor::String)
    );
    // Nested containers resolve through the trait chain.
    assert_eq!(
        <haphe::Stream<Vec<i32>> as HapheType>::DESCRIPTOR,
        TypeDescriptor::Stream(&TypeDescriptor::List(&I32))
    );
    assert_eq!(
        <Option<haphe::Stream<i32>> as HapheType>::DESCRIPTOR,
        TypeDescriptor::Option(&TypeDescriptor::Stream(&I32))
    );
}

#[test]
fn stream_values_are_usable() {
    // The wrapper carries a real stream/future, not a marker.
    struct Once(Option<i32>);
    impl haphe::futures_core::Stream for Once {
        type Item = i32;
        fn poll_next(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<i32>> {
            std::task::Poll::Ready(self.0.take())
        }
    }
    let _stream = haphe::Stream::new(Once(Some(1)));
    let _future = haphe::Future::new(async { 42 });
}
