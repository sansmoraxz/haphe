//! `#[derive(Script)]` on enums must produce exactly the descriptor a user
//! would write by hand.

#![cfg(feature = "macros")]

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
        },
        EnumVariant {
            name: "RGB",
            doc: None,
            kind: VariantKind::Tuple(&[U8, U8, U8]),
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
        },
    ],
    methods: <Color as ScriptImpl>::METHODS,
    trait_impls: &[TraitImpl::Clone],
    thread_safety: ThreadSafety::SEND_SYNC,
    generic_params: &[],
};

static GENERATED: EnumDescriptor<'static> = <Color as ScriptEnum>::DESCRIPTOR;

#[test]
fn generated_matches_hand_written() {
    assert_eq!(GENERATED, HAND_WRITTEN);
    assert_eq!(GENERATED.methods.len(), 1);
    assert_eq!(GENERATED.methods[0].name, "luminance");
    let _ = Color::Internal.clone().luminance();
}
