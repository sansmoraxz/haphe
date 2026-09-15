//! Generic type parameters fold to `TypeDescriptor::GenericParam` and are
//! recorded in `generic_params`.

#![cfg(feature = "macros")]

use haphe::{GenericParam, PrimitiveType, Script, ScriptFunction, ScriptStruct, TypeDescriptor};

/// A labeled value.
#[derive(Script)]
pub struct Labeled<T: Clone, U> {
    pub label: String,
    pub value: T,
    pub items: Vec<T>,
    pub pair: (T, U),
    pub fallback: Option<U>,
    pub count: i32,
}

#[test]
fn generic_params_recorded() {
    let desc = <Labeled<String, i32> as ScriptStruct>::DESCRIPTOR;
    assert_eq!(
        desc.generic_params,
        &[
            GenericParam {
                name: "T",
                bounds: &["Clone"],
                default: None
            },
            GenericParam {
                name: "U",
                bounds: &[],
                default: None,
            },
        ],
    );
}

#[test]
fn generic_fields_fold_syntactically() {
    let desc = <Labeled<String, i32> as ScriptStruct>::DESCRIPTOR;
    let field = |name: &str| {
        desc.fields
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("missing field {name}"))
    };
    assert_eq!(*field("label").ty, TypeDescriptor::String);
    assert_eq!(*field("value").ty, TypeDescriptor::GenericParam("T"));
    assert_eq!(
        *field("items").ty,
        TypeDescriptor::List(&TypeDescriptor::GenericParam("T")),
    );
    assert_eq!(
        *field("pair").ty,
        TypeDescriptor::Tuple(&[
            TypeDescriptor::GenericParam("T"),
            TypeDescriptor::GenericParam("U"),
        ]),
    );
    assert_eq!(
        *field("fallback").ty,
        TypeDescriptor::Option(&TypeDescriptor::GenericParam("U")),
    );
    // Types not mentioning a parameter still resolve through the trait.
    assert_eq!(
        *field("count").ty,
        TypeDescriptor::Primitive(PrimitiveType::I32)
    );
}

/// The descriptor is instantiation-independent.
#[test]
fn descriptor_is_uniform_across_instantiations() {
    assert_eq!(
        <Labeled<String, i32> as ScriptStruct>::DESCRIPTOR,
        <Labeled<bool, u8> as ScriptStruct>::DESCRIPTOR,
    );
}

mod markers {
    /// A derived type whose name shadows nothing but lives behind a module.
    #[derive(haphe::Script)]
    pub struct T {
        pub n: i32,
    }
}

/// A namespaced type merely *named* like a parameter is not a parameter.
#[derive(Script)]
pub struct Shadowed<T> {
    pub value: T,
    pub concrete: markers::T,
}

#[test]
fn namespaced_type_named_like_param_resolves_via_trait() {
    let desc = <Shadowed<i32> as ScriptStruct>::DESCRIPTOR;
    assert_eq!(*desc.fields[0].ty, TypeDescriptor::GenericParam("T"));
    assert!(matches!(*desc.fields[1].ty, TypeDescriptor::Ref(_)));
}

/// Claims on generic types are verified per instantiation through bounds on
/// the descriptor impl — a valid instantiation is usable.
#[derive(Script, Clone)]
#[script(thread_safety = send_sync, traits(Clone))]
pub struct Pair<T: Clone> {
    pub first: T,
    pub second: T,
}

#[test]
fn generic_claims_hold_for_valid_instantiations() {
    static DESC: haphe::StructDescriptor<'static> = <Pair<i32> as ScriptStruct>::DESCRIPTOR;
    assert_eq!(DESC.thread_safety, haphe::ThreadSafety::SEND_SYNC);
}

// ---------------------------------------------------------------------------
// Lifetime parameters
// ---------------------------------------------------------------------------

#[derive(Script)]
pub struct Borrowed<'a> {
    pub name: &'a str,
    pub age: i32,
}

#[test]
fn lifetime_struct_descriptor() {
    let desc = <Borrowed<'_> as ScriptStruct>::DESCRIPTOR;
    // &'a str erases to &str → String descriptor
    assert_eq!(*desc.fields[0].ty, TypeDescriptor::String);
    assert_eq!(
        *desc.fields[1].ty,
        TypeDescriptor::Primitive(PrimitiveType::I32)
    );
    // Lifetimes are not generic *type* params
    assert!(desc.generic_params.is_empty());
}

#[derive(Script)]
pub struct Mixed<'a, T: Clone> {
    pub label: &'a str,
    pub value: T,
}

#[test]
fn mixed_lifetime_and_type_param() {
    let desc = <Mixed<'_, i32> as ScriptStruct>::DESCRIPTOR;
    assert_eq!(*desc.fields[0].ty, TypeDescriptor::String);
    assert_eq!(*desc.fields[1].ty, TypeDescriptor::GenericParam("T"));
    assert_eq!(desc.generic_params.len(), 1);
    assert_eq!(desc.generic_params[0].name, "T");
}

// ---------------------------------------------------------------------------
// Lifetime parameters on free functions
// ---------------------------------------------------------------------------

#[haphe::script]
fn first_word(s: &str) -> String {
    s.split_whitespace().next().unwrap_or("").to_owned()
}

#[test]
fn lifetime_free_fn_descriptor() {
    let desc = <first_word as ScriptFunction>::DESCRIPTOR;
    assert_eq!(desc.name, "first_word");
    assert_eq!(desc.params.len(), 1);
    assert_eq!(*desc.params[0].ty, TypeDescriptor::String);
    assert_eq!(*desc.return_type, TypeDescriptor::String);
}

haphe::registry! {
    /// Generic types are listed with any concrete instantiation — the
    /// descriptor is instantiation-independent.
    pub static GENERIC_REGISTRY = {
        structs: [Labeled<String, i32>],
    };
}

#[test]
fn generic_types_in_registry() {
    let validated = GENERIC_REGISTRY.validate().unwrap();
    assert_eq!(validated.structs()[0].generic_params.len(), 2);
}

/// `HapheType` references to a generic type carry the concrete arguments so
/// monomorphizing backends can substitute them.
#[test]
fn haphe_type_reference_records_instance_args() {
    use haphe::HapheType;
    let desc = <Labeled<String, i32> as HapheType>::DESCRIPTOR;
    match desc {
        TypeDescriptor::Instance { id, args } => {
            assert_eq!(id, <Labeled<String, i32> as haphe::ScriptType>::ID);
            assert_eq!(
                args,
                &[
                    TypeDescriptor::String,
                    TypeDescriptor::Primitive(PrimitiveType::I32)
                ]
            );
        }
        other => panic!("expected Instance, got: {other:?}"),
    }
}

// `registry!` records one instantiation per generic entry and dedupes the
// erased descriptor.
haphe::registry! {
    pub static MULTI_INSTANCE_REGISTRY = {
        structs: [Labeled<String, i32>, Labeled<bool, u8>],
    };
}

#[test]
fn registry_records_instantiations_and_dedupes_erased_descriptor() {
    let validated = MULTI_INSTANCE_REGISTRY.validate().unwrap();
    assert_eq!(validated.structs().len(), 1, "erased descriptor deduped");
    let insts = validated.instantiations();
    assert_eq!(insts.len(), 2);
    assert_eq!(insts[0].id, validated.structs()[0].id);
    assert_eq!(
        insts[0].args,
        &[
            TypeDescriptor::String,
            TypeDescriptor::Primitive(PrimitiveType::I32)
        ]
    );
    assert_eq!(
        insts[1].args,
        &[
            TypeDescriptor::Primitive(PrimitiveType::Bool),
            TypeDescriptor::Primitive(PrimitiveType::U8)
        ]
    );
}
