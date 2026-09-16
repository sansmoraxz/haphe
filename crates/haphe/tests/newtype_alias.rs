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

// Transparent newtypes over primitive-valued types cross the bridge as the
// inner value — a bool newtype is a native boolean to a runtime. Opaque
// newtypes get no generated conversions.
#[derive(Script)]
#[script(transparent)]
pub struct Toggle(bool);

#[derive(Script)]
#[script(transparent)]
pub struct Label {
    text: String,
}

#[test]
fn primitive_newtypes_cross_as_native_values() {
    use haphe::{FromScript, ScriptValue};

    assert!(matches!(
        ScriptValue::from(Toggle(false)),
        ScriptValue::Bool(false)
    ));
    let t = Toggle::from_script(ScriptValue::Bool(true)).unwrap();
    assert!(t.0);
    assert!(Toggle::from_script(ScriptValue::I64(1)).is_err());

    assert!(matches!(
        ScriptValue::from(Meters(1.5)),
        ScriptValue::F64(f) if f == 1.5
    ));
    // Opaque newtypes (`RawHandle`) deliberately get no conversions —
    // enforced by the compile-time absence of `From`/`FromScript` impls.
}

#[test]
fn named_single_field_string_newtype_converts() {
    use haphe::{FromScript, ScriptValue};
    assert!(matches!(
        ScriptValue::from(Label { text: "hi".into() }),
        ScriptValue::String(s) if s == "hi"
    ));
    let l = Label::from_script(ScriptValue::String("ok".into())).unwrap();
    assert_eq!(l.text, "ok");
}

// Chains of transparent newtypes delegate conversions link by link down to
// the primitive.
#[derive(Script)]
#[script(transparent)]
pub struct Altitude(Meters);

#[derive(Script)]
#[script(transparent)]
pub struct CruisingLevel(Altitude);

#[test]
fn transparent_chains_convert_to_the_primitive() {
    use haphe::{FromScript, ScriptValue};
    assert_eq!(
        <CruisingLevel as HapheType>::DESCRIPTOR,
        TypeDescriptor::Primitive(PrimitiveType::F64),
    );
    assert!(matches!(
        ScriptValue::from(CruisingLevel(Altitude(Meters(11000.0)))),
        ScriptValue::F64(f) if f == 11000.0
    ));
    let lvl = CruisingLevel::from_script(ScriptValue::F64(1.5)).unwrap();
    assert_eq!(lvl.0.0.0, 1.5);
}
