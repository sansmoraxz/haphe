//! Single-field tuple structs derive as type aliases.

#![cfg(feature = "macros")]

use haphe::{
    HapheType, PrimitiveType, Script, ScriptAlias, ScriptStruct, ScriptType, TypeAliasDescriptor,
    TypeDescriptor, TypeId,
};

/// A distance in meters.
#[derive(Script)]
#[script(transparent)]
#[allow(dead_code)]
pub struct Meters(f64);

/// An opaque handle.
#[derive(Script)]
#[script(rename = "Handle")]
#[allow(dead_code)]
pub struct RawHandle(u64);

static METERS_HAND_WRITTEN: TypeAliasDescriptor<'static> = TypeAliasDescriptor {
    id: TypeId::new("newtype_alias::Meters"),
    name: "Meters",
    doc: Some("A distance in meters."),
    inner: &TypeDescriptor::Primitive(PrimitiveType::F64),
    transparent: true,
};

#[test]
fn generated_matches_hand_written() {
    assert_eq!(<Meters as ScriptAlias>::DESCRIPTOR, METERS_HAND_WRITTEN);
}

/// A transparent alias describes as its inner type; an opaque one as a `Ref`.
#[test]
fn transparency_controls_nesting() {
    assert_eq!(
        <Meters as HapheType>::DESCRIPTOR,
        TypeDescriptor::Primitive(PrimitiveType::F64),
    );
    assert_eq!(
        <RawHandle as HapheType>::DESCRIPTOR,
        TypeDescriptor::Ref(TypeId::new("newtype_alias::RawHandle")),
    );
    let opaque = <RawHandle as ScriptAlias>::DESCRIPTOR;
    assert_eq!(opaque.name, "Handle");
    assert!(!opaque.transparent);
}

/// Aliases nest into other descriptors through `HapheType`.
#[derive(Script)]
pub struct Segment {
    pub length: Meters,
    pub handle: RawHandle,
}

haphe::registry! {
    pub static REGISTRY = {
        structs: [Segment],
        type_aliases: [Meters, RawHandle],
    };
}

#[test]
fn registry_with_aliases_validates() {
    let validated = REGISTRY.validate().expect("aliases registered");
    assert_eq!(validated.type_aliases().len(), 2);
    let desc = <Segment as ScriptStruct>::DESCRIPTOR;
    assert_eq!(
        *desc.fields[0].ty,
        TypeDescriptor::Primitive(PrimitiveType::F64)
    );
    assert_eq!(
        *desc.fields[1].ty,
        TypeDescriptor::Ref(<RawHandle as ScriptType>::ID)
    );
}

/// A single-field named struct marked `transparent` is also an alias…
#[derive(Script)]
#[script(transparent)]
#[allow(dead_code)]
pub struct Celsius {
    degrees: f64,
}

/// …while an unmarked one stays an ordinary struct.
#[derive(Script)]
pub struct Wrapper {
    pub inner: f64,
}

#[test]
fn named_single_field_structs() {
    assert_eq!(
        <Celsius as HapheType>::DESCRIPTOR,
        TypeDescriptor::Primitive(PrimitiveType::F64),
    );
    const { assert!(<Celsius as ScriptAlias>::DESCRIPTOR.transparent) };
    let desc = <Wrapper as ScriptStruct>::DESCRIPTOR;
    assert_eq!(desc.fields.len(), 1);
    assert_eq!(desc.fields[0].name, "inner");
}
