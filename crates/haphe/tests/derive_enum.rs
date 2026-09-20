//! `#[derive(Script)]` on enums must produce exactly the descriptor a user
//! would write by hand.

#![cfg(feature = "macros")]
#![allow(
    dead_code,
    reason = "fixtures are exercised through their generated descriptors and bridge wrappers, not direct calls"
)]

use haphe::{
    EnumDescriptor, EnumVariant, FieldDescriptor, PrimitiveType, Script, ScriptEnum, ScriptImpl,
    ThreadSafety, TraitImpl, TypeDescriptor, TypeId, VariantKind, script,
};

/// A color in any supported form.
#[derive(Script, Clone)]
#[script(thread_safety = send_sync, traits(Clone), methods)]
pub enum Color {
    /// Pure red.
    Red,
    #[script(rename = "RGB")]
    Rgb(u8, u8, u8),
    Named {
        /// A CSS color name.
        name: String,
        #[script(readonly)]
        alpha: f64,
    },
    #[script(skip)]
    Internal,
}

#[script]
impl Color {
    pub fn luminance(&self) -> f64 {
        0.5
    }
}

const U8: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::U8);

static HAND_WRITTEN: EnumDescriptor<'static> = EnumDescriptor {
    id: TypeId::new("derive_enum::Color"),
    name: "Color",
    doc: Some("A color in any supported form."),
    variants: &[
        EnumVariant {
            name: "Red",
            doc: Some("Pure red."),
            kind: VariantKind::Unit,
            discriminant: None,
        },
        EnumVariant {
            name: "RGB",
            doc: None,
            kind: VariantKind::Tuple(&[U8, U8, U8]),
            discriminant: None,
        },
        EnumVariant {
            name: "Named",
            doc: None,
            kind: VariantKind::Struct(&[
                FieldDescriptor {
                    name: "name",
                    doc: Some("A CSS color name."),
                    ty: &TypeDescriptor::String,
                    readonly: false,
                },
                FieldDescriptor {
                    name: "alpha",
                    doc: None,
                    ty: &TypeDescriptor::Primitive(PrimitiveType::F64),
                    readonly: true,
                },
            ]),
            discriminant: None,
        },
    ],
    methods: <Color as ScriptImpl>::METHODS,
    trait_impls: &[TraitImpl::Clone],
    thread_safety: ThreadSafety::SEND_SYNC,
    generic_params: &[],
    repr: None,
    is_flags: false,
};

static GENERATED: EnumDescriptor<'static> = <Color as ScriptEnum>::DESCRIPTOR;

#[test]
fn generated_matches_hand_written() {
    assert_eq!(GENERATED, HAND_WRITTEN);
    assert_eq!(GENERATED.methods.len(), 1);
    assert_eq!(GENERATED.methods[0].name, "luminance");
    let _ = Color::Internal.clone().luminance();
}

/// File permission bits.
#[derive(Script)]
#[script(flags)]
enum Perm {
    Read,
    Write,
    Exec,
}

#[test]
fn flags_enum_descriptor() {
    let desc = <Perm as ScriptEnum>::DESCRIPTOR;
    assert!(desc.is_flags);
    assert_eq!(desc.variants.len(), 3);
    assert!(
        desc.variants
            .iter()
            .all(|v| matches!(v.kind, haphe::VariantKind::Unit))
    );
}

/// `i64::MIN` as an explicit discriminant is recorded exactly — its inner
/// literal overflows i64 before negation, which must not silently fall back
/// to the implicit chain.
#[derive(Script)]
#[repr(i64)]
enum Extremes {
    Min = -9_223_372_036_854_775_808,
    NegOne = -1,
    Max = 9_223_372_036_854_775_807,
}

#[test]
fn extreme_discriminants_record_exactly() {
    use haphe::ScriptEnum;
    let desc = <Extremes as ScriptEnum>::DESCRIPTOR;
    let discs: Vec<Option<i64>> = desc.variants.iter().map(|v| v.discriminant).collect();
    assert_eq!(
        discs,
        [Some(i64::MIN), Some(-1), Some(i64::MAX)],
        "explicit extreme discriminants must survive parsing"
    );
}
