//! Generator error paths, exercised with hand-built static descriptors.

use haphe::{
    FieldDescriptor, FunctionDescriptor, GenerateError, Ownership, PrimitiveType, Receiver,
    StructDescriptor, ThreadSafety, TypeDescriptor, TypeId, TypeRegistry,
};
use haphe_wit::{WitGenError, WitGenerator};

const F64: TypeDescriptor = TypeDescriptor::Primitive(PrimitiveType::F64);
const UNIT: TypeDescriptor = TypeDescriptor::Unit;

const fn plain_struct(id: &'static str, name: &'static str) -> StructDescriptor<'static> {
    StructDescriptor {
        id: TypeId::new(id),
        name,
        doc: None,
        fields: &[],
        methods: &[],
        constructors: &[],
        properties: &[],
        trait_impls: &[],
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &[],
    }
}

fn generate_err(registry: &'static TypeRegistry<'static>) -> WitGenError {
    match haphe::generate(&WitGenerator::new("haphe:demo"), registry) {
        Err(GenerateError::Backend(e)) => e,
        other => panic!("expected backend error, got: {other:?}"),
    }
}

#[test]
fn i128_is_unrepresentable() {
    static I128: TypeDescriptor = TypeDescriptor::Primitive(PrimitiveType::I128);
    static FIELDS: [FieldDescriptor; 1] = [FieldDescriptor {
        name: "big",
        doc: None,
        ty: &I128,
        readonly: false,
    }];
    static STRUCTS: [StructDescriptor; 1] = [StructDescriptor {
        fields: &FIELDS,
        ..plain_struct("test::Big", "Big")
    }];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[], &[], &[]);

    let err = generate_err(&REGISTRY);
    assert!(
        matches!(err, WitGenError::UnrepresentableType { .. }),
        "got: {err:?}"
    );
}

#[test]
fn borrowed_resource_return_rejected() {
    static POINT_REF: TypeDescriptor = TypeDescriptor::Ref(TypeId::new("test::Point"));
    static METHODS: [FunctionDescriptor; 1] = [FunctionDescriptor {
        name: "inner",
        doc: None,
        receiver: Some(Receiver::Ref),
        generic_params: &[],
        instantiations: &[],
        dispatch: haphe::Dispatch::Static,
        params: &[],
        return_type: &POINT_REF,
        return_ownership: Ownership::Ref,
        is_async: false,
        error_kind: None,
    }];
    static STRUCTS: [StructDescriptor; 1] = [StructDescriptor {
        methods: &METHODS,
        ..plain_struct("test::Point", "Point")
    }];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[], &[], &[]);

    let err = generate_err(&REGISTRY);
    assert!(
        matches!(err, WitGenError::BorrowedResourceReturn { .. }),
        "got: {err:?}"
    );
}

#[test]
fn kebab_collision_rejected() {
    static STRUCTS: [StructDescriptor; 2] = [
        plain_struct("test::MyType", "MyType"),
        plain_struct("test::my_type", "my_type"),
    ];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[], &[], &[]);

    let err = generate_err(&REGISTRY);
    assert!(
        matches!(err, WitGenError::NameCollision { .. }),
        "got: {err:?}"
    );
}

#[test]
fn empty_tuple_rejected() {
    static EMPTY_TUPLE: TypeDescriptor = TypeDescriptor::Tuple(&[]);
    static FIELDS: [FieldDescriptor; 1] = [FieldDescriptor {
        name: "nothing",
        doc: None,
        ty: &EMPTY_TUPLE,
        readonly: false,
    }];
    static STRUCTS: [StructDescriptor; 1] = [StructDescriptor {
        fields: &FIELDS,
        ..plain_struct("test::Weird", "Weird")
    }];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[], &[], &[]);

    let err = generate_err(&REGISTRY);
    assert!(
        matches!(err, WitGenError::UnrepresentableType { .. }),
        "got: {err:?}"
    );
}

#[test]
fn callback_rejected_by_capabilities() {
    static CALLBACK: TypeDescriptor = TypeDescriptor::Callback {
        params: &[F64],
        return_type: &UNIT,
    };
    static FIELDS: [FieldDescriptor; 1] = [FieldDescriptor {
        name: "on-change",
        doc: None,
        ty: &CALLBACK,
        readonly: false,
    }];
    static STRUCTS: [StructDescriptor; 1] = [StructDescriptor {
        fields: &FIELDS,
        ..plain_struct("test::Widget", "Widget")
    }];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[], &[], &[]);

    match haphe::generate(&WitGenerator::new("haphe:demo"), &REGISTRY) {
        Err(GenerateError::Incompatible(_)) => {}
        other => panic!("expected Incompatible, got: {other:?}"),
    }
}

#[test]
fn async_constructor_becomes_static_func() {
    static CTORS: [FunctionDescriptor; 2] = [
        FunctionDescriptor {
            name: "connect",
            doc: None,
            receiver: None,
            generic_params: &[],
            instantiations: &[],
            dispatch: haphe::Dispatch::Static,
            params: &[],
            return_type: &UNIT,
            return_ownership: Ownership::Owned,
            is_async: true,
            error_kind: None,
        },
        FunctionDescriptor {
            name: "new",
            doc: None,
            receiver: None,
            generic_params: &[],
            instantiations: &[],
            dispatch: haphe::Dispatch::Static,
            params: &[],
            return_type: &UNIT,
            return_ownership: Ownership::Owned,
            is_async: false,
            error_kind: None,
        },
    ];
    static STRUCTS: [StructDescriptor; 1] = [StructDescriptor {
        constructors: &CTORS,
        ..plain_struct("test::Client", "Client")
    }];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[], &[], &[]);

    let output = haphe::generate(&WitGenerator::new("haphe:demo"), &REGISTRY).unwrap();
    let wit = String::from_utf8(output.files[0].content.clone()).unwrap();
    // WIT constructors cannot be async: the async one becomes a static func
    // and the first sync one takes the constructor slot.
    assert!(
        wit.contains("connect: static async func() -> client;"),
        "got:\n{wit}"
    );
    assert!(wit.contains("constructor();"), "got:\n{wit}");
}

#[test]
fn empty_enum_rejected() {
    static ENUMS: [haphe::EnumDescriptor; 1] = [haphe::EnumDescriptor {
        id: TypeId::new("test::Never"),
        name: "Never",
        doc: None,
        variants: &[],
        methods: &[],
        trait_impls: &[],
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &[],
        repr: None,
        is_flags: false,
    }];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&[], &ENUMS, &[], &[], &[], &[]);

    let err = generate_err(&REGISTRY);
    assert!(
        matches!(err, WitGenError::InvalidWit { .. }),
        "got: {err:?}"
    );
}

#[test]
fn flags_with_gap_bearing_bits_rejected() {
    // Bits 0 and 2 (values 1 and 4): WIT flags are position-based and would
    // assign bit 1 (value 2) to the second flag — a misrepresented mask.
    static VARIANTS: [haphe::EnumVariant; 2] = [
        haphe::EnumVariant {
            name: "READ",
            doc: None,
            kind: haphe::VariantKind::Unit,
            discriminant: Some(1),
        },
        haphe::EnumVariant {
            name: "EXEC",
            doc: None,
            kind: haphe::VariantKind::Unit,
            discriminant: Some(4),
        },
    ];
    static ENUMS: [haphe::EnumDescriptor; 1] = [haphe::EnumDescriptor {
        id: TypeId::new("test::GappyPerm"),
        name: "GappyPerm",
        doc: None,
        variants: &VARIANTS,
        methods: &[],
        trait_impls: &[],
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &[],
        repr: Some(haphe::PrimitiveType::U8),
        is_flags: true,
    }];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&[], &ENUMS, &[], &[], &[], &[]);

    let err = generate_err(&REGISTRY);
    match err {
        WitGenError::FlagsBitMismatch {
            name,
            flag,
            expected,
            actual,
        } => {
            assert_eq!(name, "GappyPerm");
            assert_eq!(flag, "EXEC");
            assert_eq!(expected, 2);
            assert_eq!(actual, 4);
        }
        other => panic!("expected FlagsBitMismatch, got: {other:?}"),
    }
}

#[test]
fn recursive_record_rejected() {
    static NODE_REF: TypeDescriptor = TypeDescriptor::Ref(TypeId::new("test::Node"));
    static NODE_OPT: TypeDescriptor = TypeDescriptor::Option(&NODE_REF);
    static FIELDS: [FieldDescriptor; 1] = [FieldDescriptor {
        name: "next",
        doc: None,
        ty: &NODE_OPT,
        readonly: false,
    }];
    static STRUCTS: [StructDescriptor; 1] = [StructDescriptor {
        fields: &FIELDS,
        ..plain_struct("test::Node", "Node")
    }];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[], &[], &[]);

    let err = generate_err(&REGISTRY);
    assert!(
        matches!(err, WitGenError::InvalidWit { .. }),
        "got: {err:?}"
    );
}

#[test]
fn mutually_recursive_records_rejected() {
    static A_REF: TypeDescriptor = TypeDescriptor::Ref(TypeId::new("test::A"));
    static B_REF: TypeDescriptor = TypeDescriptor::Ref(TypeId::new("test::B"));
    static B_LIST: TypeDescriptor = TypeDescriptor::List(&B_REF);
    static A_FIELDS: [FieldDescriptor; 1] = [FieldDescriptor {
        name: "children",
        doc: None,
        ty: &B_LIST,
        readonly: false,
    }];
    static B_FIELDS: [FieldDescriptor; 1] = [FieldDescriptor {
        name: "parent",
        doc: None,
        ty: &A_REF,
        readonly: false,
    }];
    static STRUCTS: [StructDescriptor; 2] = [
        StructDescriptor {
            fields: &A_FIELDS,
            ..plain_struct("test::A", "A")
        },
        StructDescriptor {
            fields: &B_FIELDS,
            ..plain_struct("test::B", "B")
        },
    ];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[], &[], &[]);

    let err = generate_err(&REGISTRY);
    assert!(
        matches!(err, WitGenError::InvalidWit { .. }),
        "got: {err:?}"
    );
}

#[test]
fn cyclic_interface_use_rejected() {
    // Two resources in different modules whose methods reference each other:
    // no value-type recursion (handles break it), but the interfaces `use`
    // each other cyclically.
    static R1_REF: TypeDescriptor = TypeDescriptor::Ref(TypeId::new("test::R1"));
    static R2_REF: TypeDescriptor = TypeDescriptor::Ref(TypeId::new("test::R2"));
    const fn method(
        name: &'static str,
        param_ty: &'static TypeDescriptor<'static>,
    ) -> FunctionDescriptor<'static> {
        FunctionDescriptor {
            name,
            doc: None,
            receiver: Some(Receiver::Ref),
            generic_params: &[],
            instantiations: &[],
            dispatch: haphe::Dispatch::Static,
            params: &[],
            return_type: param_ty,
            return_ownership: Ownership::Owned,
            is_async: false,
            error_kind: None,
        }
    }
    static R1_METHODS: [FunctionDescriptor; 1] = [method("other", &R2_REF)];
    static R2_METHODS: [FunctionDescriptor; 1] = [method("other", &R1_REF)];
    static STRUCTS: [StructDescriptor; 2] = [
        StructDescriptor {
            methods: &R1_METHODS,
            ..plain_struct("test::R1", "R1")
        },
        StructDescriptor {
            methods: &R2_METHODS,
            ..plain_struct("test::R2", "R2")
        },
    ];
    static MODULES: [haphe::ModuleDescriptor; 2] = [
        haphe::ModuleDescriptor {
            name: "alpha",
            doc: None,
            functions: &[],
            type_ids: &[TypeId::new("test::R1")],
            submodules: &[],
            constants: &[],
            function_instantiations: &[],
        },
        haphe::ModuleDescriptor {
            name: "beta",
            doc: None,
            functions: &[],
            type_ids: &[TypeId::new("test::R2")],
            submodules: &[],
            constants: &[],
            function_instantiations: &[],
        },
    ];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &MODULES, &[], &[]);

    let err = generate_err(&REGISTRY);
    assert!(
        matches!(err, WitGenError::InvalidWit { .. }),
        "got: {err:?}"
    );
}

const fn generic_struct(
    id: &'static str,
    name: &'static str,
    fields: &'static [FieldDescriptor<'static>],
) -> StructDescriptor<'static> {
    StructDescriptor {
        fields,
        generic_params: &[haphe::GenericParam {
            name: "T",
            bounds: &[],
            default: None,
        }],
        ..plain_struct(id, name)
    }
}

static T_PARAM: TypeDescriptor = TypeDescriptor::GenericParam("T");
static GENERIC_FIELDS: [FieldDescriptor; 1] = [FieldDescriptor {
    name: "value",
    doc: None,
    ty: &T_PARAM,
    readonly: false,
}];

#[test]
fn uninstantiated_generic_rejected() {
    static STRUCTS: [StructDescriptor; 1] =
        [generic_struct("test::Holder", "Holder", &GENERIC_FIELDS)];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[], &[], &[]);

    match haphe::generate(&WitGenerator::new("haphe:demo"), &REGISTRY) {
        Err(GenerateError::Incompatible(errors)) => {
            assert!(
                errors
                    .iter()
                    .any(|e| matches!(e, haphe::CompatibilityError::UninstantiatedGeneric { .. })),
                "got: {errors:?}"
            );
        }
        other => panic!("expected Incompatible, got: {other:?}"),
    }
}

#[test]
fn unregistered_instantiation_rejected() {
    // `User` references Holder<f64>, but only Holder<bool> is registered.
    static F64_ARGS: [TypeDescriptor; 1] = [F64];
    static BOOL_ARGS: [TypeDescriptor; 1] = [TypeDescriptor::Primitive(PrimitiveType::Bool)];
    static HOLDER_F64: TypeDescriptor = TypeDescriptor::Instance {
        id: TypeId::new("test::Holder"),
        args: &F64_ARGS,
    };
    static USER_FIELDS: [FieldDescriptor; 1] = [FieldDescriptor {
        name: "holder",
        doc: None,
        ty: &HOLDER_F64,
        readonly: false,
    }];
    static STRUCTS: [StructDescriptor; 2] = [
        generic_struct("test::Holder", "Holder", &GENERIC_FIELDS),
        StructDescriptor {
            fields: &USER_FIELDS,
            ..plain_struct("test::User", "User")
        },
    ];
    static INSTANTIATIONS: [haphe::InstantiationDescriptor; 1] = [haphe::InstantiationDescriptor {
        id: TypeId::new("test::Holder"),
        args: &BOOL_ARGS,
    }];
    static REGISTRY: TypeRegistry =
        TypeRegistry::new(&STRUCTS, &[], &[], &[], &INSTANTIATIONS, &[]);

    let err = generate_err(&REGISTRY);
    assert!(
        matches!(err, WitGenError::UnregisteredInstantiation { ref name } if name == "holder-f64"),
        "got: {err:?}"
    );
}

#[test]
fn mangled_name_collision_rejected() {
    // `Holder<bool>` mangles to `holder-bool`, colliding with the concrete
    // struct `HolderBool`.
    static BOOL_ARGS: [TypeDescriptor; 1] = [TypeDescriptor::Primitive(PrimitiveType::Bool)];
    static CONCRETE_FIELDS: [FieldDescriptor; 1] = [FieldDescriptor {
        name: "flag",
        doc: None,
        ty: &TypeDescriptor::Primitive(PrimitiveType::Bool),
        readonly: false,
    }];
    static STRUCTS: [StructDescriptor; 2] = [
        generic_struct("test::Holder", "Holder", &GENERIC_FIELDS),
        StructDescriptor {
            fields: &CONCRETE_FIELDS,
            ..plain_struct("test::HolderBool", "HolderBool")
        },
    ];
    static INSTANTIATIONS: [haphe::InstantiationDescriptor; 1] = [haphe::InstantiationDescriptor {
        id: TypeId::new("test::Holder"),
        args: &BOOL_ARGS,
    }];
    static REGISTRY: TypeRegistry =
        TypeRegistry::new(&STRUCTS, &[], &[], &[], &INSTANTIATIONS, &[]);

    let err = generate_err(&REGISTRY);
    assert!(
        matches!(err, WitGenError::NameCollision { .. }),
        "got: {err:?}"
    );
}

#[test]
fn instantiation_arity_mismatch_rejected_at_validation() {
    static TWO_ARGS: [TypeDescriptor; 2] = [F64, F64];
    static STRUCTS: [StructDescriptor; 1] =
        [generic_struct("test::Holder", "Holder", &GENERIC_FIELDS)];
    static INSTANTIATIONS: [haphe::InstantiationDescriptor; 1] = [haphe::InstantiationDescriptor {
        id: TypeId::new("test::Holder"),
        args: &TWO_ARGS,
    }];
    static REGISTRY: TypeRegistry =
        TypeRegistry::new(&STRUCTS, &[], &[], &[], &INSTANTIATIONS, &[]);

    match haphe::generate(&WitGenerator::new("haphe:demo"), &REGISTRY) {
        Err(GenerateError::Invalid(errors)) => {
            assert!(
                errors
                    .iter()
                    .any(|e| matches!(e, haphe::RegistryError::InstantiationArityMismatch { .. })),
                "got: {errors:?}"
            );
        }
        other => panic!("expected Invalid, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Foreign interfaces
// ---------------------------------------------------------------------------

const fn foreign_fn(
    name: &'static str,
    generic_params: &'static [haphe::GenericParam<'static>],
    instantiations: &'static [&'static [TypeDescriptor<'static>]],
    return_type: &'static TypeDescriptor<'static>,
) -> FunctionDescriptor<'static> {
    FunctionDescriptor {
        name,
        doc: None,
        receiver: Some(Receiver::Ref),
        generic_params,
        instantiations,
        dispatch: haphe::Dispatch::Static,
        params: &[],
        return_type,
        return_ownership: Ownership::Owned,
        is_async: false,
        error_kind: None,
    }
}

const fn foreign_iface(
    id: &'static str,
    name: &'static str,
    generic_params: &'static [haphe::GenericParam<'static>],
    functions: &'static [FunctionDescriptor<'static>],
) -> haphe::ForeignInterfaceDescriptor<'static> {
    haphe::ForeignInterfaceDescriptor {
        id: TypeId::new(id),
        name,
        doc: None,
        generic_params,
        functions,
        thread_safety: ThreadSafety::NONE,
    }
}

#[test]
fn i128_in_foreign_signature_is_unrepresentable() {
    static I128: TypeDescriptor = TypeDescriptor::Primitive(PrimitiveType::I128);
    static FNS: [FunctionDescriptor; 1] = [foreign_fn("big", &[], &[], &I128)];
    static FOREIGN: [haphe::ForeignInterfaceDescriptor; 1] =
        [foreign_iface("test::Hooks", "Hooks", &[], &FNS)];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&[], &[], &[], &[], &[], &FOREIGN);

    let err = generate_err(&REGISTRY);
    assert!(
        matches!(err, WitGenError::UnrepresentableType { .. }),
        "got: {err:?}"
    );
}

#[test]
fn foreign_interface_name_collision_with_module() {
    static FNS: [FunctionDescriptor; 1] = [foreign_fn("ping", &[], &[], &UNIT)];
    static FOREIGN: [haphe::ForeignInterfaceDescriptor; 1] =
        [foreign_iface("test::Geometry", "Geometry", &[], &FNS)];
    static MODULES: [haphe::ModuleDescriptor; 1] = [haphe::ModuleDescriptor {
        name: "geometry",
        doc: None,
        functions: &[],
        type_ids: &[],
        submodules: &[],
        constants: &[],
        function_instantiations: &[],
    }];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&[], &[], &[], &MODULES, &[], &FOREIGN);

    let err = generate_err(&REGISTRY);
    assert!(
        matches!(err, WitGenError::NameCollision { .. }),
        "got: {err:?}"
    );
}

#[cfg_attr(feature = "generics", allow(dead_code))]
static FGN_T_PARAM: [haphe::GenericParam<'static>; 1] = [haphe::GenericParam {
    name: "T",
    bounds: &[],
    default: None,
}];
#[cfg_attr(feature = "generics", allow(dead_code))]
static FGN_T_TYPE: TypeDescriptor = TypeDescriptor::GenericParam("T");
#[cfg_attr(feature = "generics", allow(dead_code))]
static S64_ARGS: [TypeDescriptor; 1] = [TypeDescriptor::Primitive(PrimitiveType::I64)];

/// Without the `generics` feature, generic foreign interfaces and generic
/// functions are rejected descriptively rather than emitted.
#[cfg(not(feature = "generics"))]
#[test]
fn generic_foreign_interface_rejected_without_feature() {
    static FNS: [FunctionDescriptor; 1] = [foreign_fn("get", &[], &[], &FGN_T_TYPE)];
    static FOREIGN: [haphe::ForeignInterfaceDescriptor; 1] =
        [foreign_iface("test::Store", "Store", &FGN_T_PARAM, &FNS)];
    static INSTS: [haphe::InstantiationDescriptor; 1] = [haphe::InstantiationDescriptor {
        id: TypeId::new("test::Store"),
        args: &S64_ARGS,
    }];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&[], &[], &[], &[], &INSTS, &FOREIGN);

    let err = generate_err(&REGISTRY);
    assert!(
        matches!(err, WitGenError::GenericForeignInterface { .. }),
        "got: {err:?}"
    );
}

#[cfg(not(feature = "generics"))]
#[test]
fn generic_foreign_function_rejected_without_feature() {
    static INSTS: [&[TypeDescriptor]; 1] = [&S64_ARGS];
    static FNS: [FunctionDescriptor; 1] = [foreign_fn("get", &FGN_T_PARAM, &INSTS, &FGN_T_TYPE)];
    static FOREIGN: [haphe::ForeignInterfaceDescriptor; 1] =
        [foreign_iface("test::Hooks", "Hooks", &[], &FNS)];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&[], &[], &[], &[], &[], &FOREIGN);

    let err = generate_err(&REGISTRY);
    assert!(
        matches!(err, WitGenError::GenericFunction { .. }),
        "got: {err:?}"
    );
}

/// A generic module function whose only instantiations are registry-level
/// is still rejected without the `generics` feature — the gate keys on the
/// declared generic parameters, not on where instantiations come from.
#[cfg(not(feature = "generics"))]
#[test]
fn registry_instantiated_generic_fn_rejected_without_feature() {
    static FNS: [FunctionDescriptor; 1] = [foreign_fn("relay", &FGN_T_PARAM, &[], &FGN_T_TYPE)];
    static INSTS: [haphe::FnInstantiation; 1] = [haphe::FnInstantiation {
        function: "relay",
        args: &S64_ARGS,
    }];
    static MODULES: [haphe::ModuleDescriptor; 1] = [haphe::ModuleDescriptor {
        name: "util",
        doc: None,
        functions: &FNS,
        type_ids: &[],
        submodules: &[],
        constants: &[],
        function_instantiations: &INSTS,
    }];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&[], &[], &[], &MODULES, &[], &[]);

    let err = generate_err(&REGISTRY);
    assert!(
        matches!(err, WitGenError::GenericFunction { .. }),
        "got: {err:?}"
    );
}

/// A user method named `at` collides with the `Index` trait projection —
/// same per-resource NameMap, descriptive error.
#[test]
fn projection_name_collision_with_user_method() {
    static AT_METHOD: [FunctionDescriptor; 1] = [FunctionDescriptor {
        name: "at",
        doc: None,
        receiver: Some(Receiver::Ref),
        generic_params: &[],
        instantiations: &[],
        dispatch: haphe::Dispatch::Static,
        params: &[],
        return_type: &F64,
        return_ownership: Ownership::Owned,
        is_async: false,
        error_kind: None,
    }];
    static S64: TypeDescriptor = TypeDescriptor::Primitive(PrimitiveType::I64);
    static TRAITS: [haphe::TraitImpl; 1] = [haphe::TraitImpl::Index {
        index: &S64,
        output: &S64,
    }];
    static STRUCTS: [StructDescriptor; 1] = [StructDescriptor {
        methods: &AT_METHOD,
        trait_impls: &TRAITS,
        ..plain_struct("t::Grid", "Grid")
    }];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[], &[], &[]);

    let err = generate_err(&REGISTRY);
    assert!(
        matches!(err, WitGenError::NameCollision { ref kebab, .. } if kebab == "at"),
        "got: {err:?}"
    );
}

/// Declaring both `Call` and `AsyncCall` cannot project uniquely.
#[test]
fn both_call_traits_rejected_in_projection() {
    static S64: TypeDescriptor = TypeDescriptor::Primitive(PrimitiveType::I64);
    static ARGS: [TypeDescriptor; 1] = [S64];
    static TRAITS: [haphe::TraitImpl; 2] = [
        haphe::TraitImpl::Call {
            args: &ARGS,
            output: &S64,
        },
        haphe::TraitImpl::AsyncCall {
            args: &ARGS,
            output: &S64,
        },
    ];
    static CTOR: [FunctionDescriptor; 1] = [FunctionDescriptor {
        name: "new",
        doc: None,
        receiver: None,
        generic_params: &[],
        instantiations: &[],
        dispatch: haphe::Dispatch::Static,
        params: &[],
        return_type: &UNIT,
        return_ownership: Ownership::Owned,
        is_async: false,
        error_kind: None,
    }];
    static STRUCTS: [StructDescriptor; 1] = [StructDescriptor {
        constructors: &CTOR,
        trait_impls: &TRAITS,
        ..plain_struct("t::Fx", "Fx")
    }];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[], &[], &[]);

    let err = generate_err(&REGISTRY);
    assert!(
        matches!(err, WitGenError::UnrepresentableType { ref detail, .. }
            if detail.contains("both `Call` and `AsyncCall`")),
        "got: {err:?}"
    );
}

// ── Borrowed (lifetime-carrying) descriptors

const fn plain_string_fn(
    params: &'static [haphe::ParamDescriptor<'static>],
    ty: &'static TypeDescriptor<'static>,
) -> FunctionDescriptor<'static> {
    FunctionDescriptor {
        name: "shout",
        doc: None,
        receiver: None,
        generic_params: &[],
        instantiations: &[],
        dispatch: haphe::Dispatch::Static,
        params,
        return_type: ty,
        return_ownership: Ownership::Owned,
        is_async: false,
        error_kind: None,
    }
}

/// WIT values cross by copy: a `Borrowed` descriptor (`Cow<'a, str>`) lowers
/// as its carried type and the lifetime never reaches the text — output is
/// byte-identical to the plain twin, nested occurrences included.
#[test]
fn borrowed_lowers_as_its_inner_type() {
    use haphe::BindingGenerator;

    static BORROWED: TypeDescriptor = TypeDescriptor::Borrowed {
        lifetime: Some("a"),
        inner: &TypeDescriptor::List(&TypeDescriptor::Borrowed {
            lifetime: None,
            inner: &TypeDescriptor::String,
        }),
    };
    static PLAIN: TypeDescriptor = TypeDescriptor::List(&TypeDescriptor::String);
    static BORROWED_PARAMS: [haphe::ParamDescriptor; 1] = [haphe::ParamDescriptor {
        name: "text",
        ty: &BORROWED,
        ownership: Ownership::Owned,
    }];
    static PLAIN_PARAMS: [haphe::ParamDescriptor; 1] = [haphe::ParamDescriptor {
        name: "text",
        ty: &PLAIN,
        ownership: Ownership::Owned,
    }];
    static BORROWED_FNS: [FunctionDescriptor; 1] = [plain_string_fn(&BORROWED_PARAMS, &BORROWED)];
    static PLAIN_FNS: [FunctionDescriptor; 1] = [plain_string_fn(&PLAIN_PARAMS, &PLAIN)];
    static BORROWED_MODULES: [haphe::ModuleDescriptor; 1] = [dispatch_module(&BORROWED_FNS)];
    static PLAIN_MODULES: [haphe::ModuleDescriptor; 1] = [dispatch_module(&PLAIN_FNS)];
    static BORROWED_REGISTRY: TypeRegistry =
        TypeRegistry::new(&[], &[], &[], &BORROWED_MODULES, &[], &[]);
    static PLAIN_REGISTRY: TypeRegistry =
        TypeRegistry::new(&[], &[], &[], &PLAIN_MODULES, &[], &[]);

    let generator = WitGenerator::new("haphe:demo");
    let borrowed_out = generator
        .generate(&BORROWED_REGISTRY.validate().unwrap())
        .expect("borrowed twin generates");
    let plain_out = generator
        .generate(&PLAIN_REGISTRY.validate().unwrap())
        .expect("plain twin generates");
    assert_eq!(
        borrowed_out.files[0].content, plain_out.files[0].content,
        "the lifetime must not affect emitted WIT"
    );
    let text = String::from_utf8_lossy(&borrowed_out.files[0].content);
    assert!(
        text.contains("shout: func(text: list<string>) -> list<string>;"),
        "got:\n{text}"
    );
}

// ── Dyn-dispatched generics

const fn generic_module_fn(dispatch: haphe::Dispatch) -> FunctionDescriptor<'static> {
    FunctionDescriptor {
        name: "echo",
        doc: None,
        receiver: None,
        generic_params: &[haphe::GenericParam {
            name: "T",
            bounds: &[],
            default: None,
        }],
        instantiations: &[&[TypeDescriptor::Primitive(PrimitiveType::I64)]],
        dispatch,
        params: &[haphe::ParamDescriptor {
            name: "value",
            ty: &TypeDescriptor::GenericParam("T"),
            ownership: Ownership::Owned,
        }],
        return_type: &TypeDescriptor::GenericParam("T"),
        return_ownership: Ownership::Owned,
        is_async: false,
        error_kind: None,
    }
}

const fn dispatch_module(
    functions: &'static [FunctionDescriptor<'static>],
) -> haphe::ModuleDescriptor<'static> {
    haphe::ModuleDescriptor {
        name: "util",
        doc: None,
        functions,
        type_ids: &[],
        submodules: &[],
        constants: &[],
        function_instantiations: &[],
    }
}

/// WIT guests always name a monomorph statically, so bare dyn dispatch is
/// rejected by the capability check.
#[test]
#[cfg(not(feature = "dyn-generics"))]
fn dyn_generic_fn_rejected_by_capabilities() {
    static FNS: [FunctionDescriptor; 1] = [generic_module_fn(haphe::Dispatch::Dyn)];
    static MODULES: [haphe::ModuleDescriptor; 1] = [dispatch_module(&FNS)];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&[], &[], &[], &MODULES, &[], &[]);

    match haphe::generate(&WitGenerator::new("haphe:demo"), &REGISTRY) {
        Err(GenerateError::Incompatible(errors)) => {
            assert!(
                errors.iter().any(|e| matches!(
                    e,
                    haphe::CompatibilityError::DynGenericsUnsupported { function: "echo" }
                )),
                "expected DynGenericsUnsupported, got: {errors:?}"
            );
        }
        other => panic!("expected Incompatible, got: {other:?}"),
    }
}

/// WIT text depends only on the monomorph set, never on the dispatch mode:
/// a dyn twin (built by hand and generated directly, bypassing the facade's
/// capability check) emits byte-identical output to its static sibling.
/// Generic emission requires the `generics` extension feature.
#[test]
#[cfg(all(feature = "generics", not(feature = "dyn-generics")))]
fn wit_text_is_dispatch_mode_neutral() {
    use haphe::BindingGenerator;

    static STATIC_FNS: [FunctionDescriptor; 1] = [generic_module_fn(haphe::Dispatch::Static)];
    static DYN_FNS: [FunctionDescriptor; 1] = [generic_module_fn(haphe::Dispatch::Dyn)];
    static STATIC_MODULES: [haphe::ModuleDescriptor; 1] = [dispatch_module(&STATIC_FNS)];
    static DYN_MODULES: [haphe::ModuleDescriptor; 1] = [dispatch_module(&DYN_FNS)];
    static STATIC_REGISTRY: TypeRegistry =
        TypeRegistry::new(&[], &[], &[], &STATIC_MODULES, &[], &[]);
    static DYN_REGISTRY: TypeRegistry = TypeRegistry::new(&[], &[], &[], &DYN_MODULES, &[], &[]);

    let generator = WitGenerator::new("haphe:demo");
    let static_out = generator
        .generate(&STATIC_REGISTRY.validate().unwrap())
        .expect("static twin generates");
    let dyn_out = generator
        .generate(&DYN_REGISTRY.validate().unwrap())
        .expect("dyn twin generates (direct call bypasses the capability check)");
    assert_eq!(
        static_out.files[0].content, dyn_out.files[0].content,
        "dispatch mode must not affect emitted WIT"
    );
}

// ── Dyn-dispatched generic methods

/// `dyn` generic methods on structs are rejected by the capability check,
/// like free functions: WIT guests always name a monomorph statically.
#[test]
#[cfg(not(feature = "dyn-generics"))]
fn dyn_generic_struct_method_rejected_by_capabilities() {
    static METHODS: [FunctionDescriptor; 1] = [FunctionDescriptor {
        receiver: Some(Receiver::Ref),
        ..generic_module_fn(haphe::Dispatch::Dyn)
    }];
    static STRUCTS: [StructDescriptor; 1] = [StructDescriptor {
        methods: &METHODS,
        ..plain_struct("test::Holder", "Holder")
    }];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[], &[], &[]);

    match haphe::generate(&WitGenerator::new("haphe:demo"), &REGISTRY) {
        Err(GenerateError::Incompatible(errors)) => {
            assert!(
                errors.iter().any(|e| matches!(
                    e,
                    haphe::CompatibilityError::DynGenericsUnsupported { function: "echo" }
                )),
                "expected DynGenericsUnsupported, got: {errors:?}"
            );
        }
        other => panic!("expected Incompatible, got: {other:?}"),
    }
}

/// `dyn` generic methods on enums are rejected the same way.
#[test]
#[cfg(not(feature = "dyn-generics"))]
fn dyn_generic_enum_method_rejected_by_capabilities() {
    static METHODS: [FunctionDescriptor; 1] = [FunctionDescriptor {
        receiver: Some(Receiver::Ref),
        ..generic_module_fn(haphe::Dispatch::Dyn)
    }];
    static VARIANTS: [haphe::EnumVariant; 1] = [haphe::EnumVariant {
        name: "On",
        doc: None,
        kind: haphe::VariantKind::Unit,
        discriminant: None,
    }];
    static ENUMS: [haphe::EnumDescriptor; 1] = [haphe::EnumDescriptor {
        id: TypeId::new("test::Toggle"),
        name: "Toggle",
        doc: None,
        variants: &VARIANTS,
        methods: &METHODS,
        trait_impls: &[],
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &[],
        repr: None,
        is_flags: false,
    }];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&[], &ENUMS, &[], &[], &[], &[]);

    match haphe::generate(&WitGenerator::new("haphe:demo"), &REGISTRY) {
        Err(GenerateError::Incompatible(errors)) => {
            assert!(
                errors.iter().any(|e| matches!(
                    e,
                    haphe::CompatibilityError::DynGenericsUnsupported { function: "echo" }
                )),
                "expected DynGenericsUnsupported, got: {errors:?}"
            );
        }
        other => panic!("expected Incompatible, got: {other:?}"),
    }
}

/// Dispatch-mode neutrality holds for methods too: static-vs-dyn twins of a
/// generic struct method produce the same generation outcome byte for byte
/// (direct call bypasses the facade's capability check).
#[test]
#[cfg(all(feature = "generics", not(feature = "dyn-generics")))]
fn wit_method_text_is_dispatch_mode_neutral() {
    use haphe::BindingGenerator;

    static STATIC_METHODS: [FunctionDescriptor; 1] = [FunctionDescriptor {
        receiver: Some(Receiver::Ref),
        ..generic_module_fn(haphe::Dispatch::Static)
    }];
    static DYN_METHODS: [FunctionDescriptor; 1] = [FunctionDescriptor {
        receiver: Some(Receiver::Ref),
        ..generic_module_fn(haphe::Dispatch::Dyn)
    }];
    static STATIC_STRUCTS: [StructDescriptor; 1] = [StructDescriptor {
        methods: &STATIC_METHODS,
        ..plain_struct("test::Holder", "Holder")
    }];
    static DYN_STRUCTS: [StructDescriptor; 1] = [StructDescriptor {
        methods: &DYN_METHODS,
        ..plain_struct("test::Holder", "Holder")
    }];
    static STATIC_REGISTRY: TypeRegistry =
        TypeRegistry::new(&STATIC_STRUCTS, &[], &[], &[], &[], &[]);
    static DYN_REGISTRY: TypeRegistry = TypeRegistry::new(&DYN_STRUCTS, &[], &[], &[], &[], &[]);

    let generator = WitGenerator::new("haphe:demo");
    let static_out = generator.generate(&STATIC_REGISTRY.validate().unwrap());
    let dyn_out = generator.generate(&DYN_REGISTRY.validate().unwrap());
    match (static_out, dyn_out) {
        (Ok(a), Ok(b)) => assert_eq!(
            a.files[0].content, b.files[0].content,
            "dispatch mode must not affect emitted WIT"
        ),
        (Err(a), Err(b)) => assert_eq!(
            format!("{a:?}"),
            format!("{b:?}"),
            "dispatch mode must not affect the generation outcome"
        ),
        (a, b) => panic!("outcomes diverged by dispatch mode: {a:?} vs {b:?}"),
    }
}

// ── Injected dyn dispatchers (feature `dyn-generics`)

/// With the `dyn-generics` feature the capability flips on and the facade
/// generates dyn registries instead of rejecting them; the output carries
/// the static monomorphs PLUS one synthesized variant-based dispatcher.
#[test]
#[cfg(feature = "dyn-generics")]
fn dyn_generic_fn_emits_a_dispatcher() {
    use haphe::BindingGenerator;

    static INSTS: [&[TypeDescriptor]; 2] = [
        &[TypeDescriptor::Primitive(PrimitiveType::I64)],
        &[TypeDescriptor::String],
    ];
    static FNS: [FunctionDescriptor; 1] = [FunctionDescriptor {
        instantiations: &INSTS,
        ..generic_module_fn(haphe::Dispatch::Dyn)
    }];
    static MODULES: [haphe::ModuleDescriptor; 1] = [dispatch_module(&FNS)];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&[], &[], &[], &MODULES, &[], &[]);

    assert!(
        WitGenerator::new("haphe:demo").capabilities().dyn_generics,
        "capability must report true with the feature compiled in"
    );
    let out = haphe::generate(&WitGenerator::new("haphe:demo"), &REGISTRY)
        .expect("dyn registries generate with the feature on");
    let text = String::from_utf8_lossy(&out.files[0].content);
    // The static monomorphs remain (additive).
    assert!(
        text.contains("echo-s64: func(value: s64) -> s64;"),
        "got:\n{text}"
    );
    assert!(
        text.contains("echo-string: func(value: string) -> string;"),
        "got:\n{text}"
    );
    // One shape-deduplicated variant serves both the parameter and the
    // return; keyword-shaped case names are `%`-escaped in source.
    assert!(text.contains("variant echo-dyn-value {"), "got:\n{text}");
    assert!(text.contains("%s64(s64),"), "got:\n{text}");
    assert!(text.contains("%string(string),"), "got:\n{text}");
    assert!(
        text.contains("/// haphe:dyn-dispatcher = echo"),
        "got:\n{text}"
    );
    assert!(
        text.contains("echo-dyn: func(value: echo-dyn-value) -> echo-dyn-value;"),
        "got:\n{text}"
    );
}

/// A dyn METHOD declares its dispatcher as a resource member, the variant
/// types at interface level (resource bodies hold only functions).
#[test]
#[cfg(feature = "dyn-generics")]
fn dyn_generic_method_emits_a_resource_dispatcher() {
    static INSTS: [&[TypeDescriptor]; 2] = [
        &[TypeDescriptor::Primitive(PrimitiveType::I64)],
        &[TypeDescriptor::String],
    ];
    static METHODS: [FunctionDescriptor; 1] = [FunctionDescriptor {
        receiver: Some(Receiver::Ref),
        instantiations: &INSTS,
        ..generic_module_fn(haphe::Dispatch::Dyn)
    }];
    static STRUCTS: [StructDescriptor; 1] = [StructDescriptor {
        methods: &METHODS,
        ..plain_struct("test::Holder", "Holder")
    }];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[], &[], &[]);

    let out = haphe::generate(&WitGenerator::new("haphe:demo"), &REGISTRY)
        .expect("dyn method registries generate with the feature on");
    let text = String::from_utf8_lossy(&out.files[0].content);
    assert!(
        text.contains("variant holder-echo-dyn-value {"),
        "got:\n{text}"
    );
    assert!(
        text.contains("echo-dyn: func(value: holder-echo-dyn-value) -> holder-echo-dyn-value;"),
        "got:\n{text}"
    );
    // The variant is defined before the resource block that uses it.
    let variant_at = text.find("variant holder-echo-dyn-value").unwrap();
    let resource_at = text.find("resource holder").unwrap();
    assert!(variant_at < resource_at, "got:\n{text}");
}

/// A dyn ENUM method declares its dispatcher as an interface-level companion
/// taking `this` first.
#[test]
#[cfg(feature = "dyn-generics")]
fn dyn_generic_enum_method_emits_a_companion_dispatcher() {
    static INSTS: [&[TypeDescriptor]; 2] = [
        &[TypeDescriptor::Primitive(PrimitiveType::I64)],
        &[TypeDescriptor::String],
    ];
    static METHODS: [FunctionDescriptor; 1] = [FunctionDescriptor {
        receiver: Some(Receiver::Ref),
        instantiations: &INSTS,
        ..generic_module_fn(haphe::Dispatch::Dyn)
    }];
    static VARIANTS: [haphe::EnumVariant; 1] = [haphe::EnumVariant {
        name: "On",
        doc: None,
        kind: haphe::VariantKind::Unit,
        discriminant: None,
    }];
    static ENUMS: [haphe::EnumDescriptor; 1] = [haphe::EnumDescriptor {
        id: TypeId::new("test::Toggle"),
        name: "Toggle",
        doc: None,
        variants: &VARIANTS,
        methods: &METHODS,
        trait_impls: &[],
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &[],
        repr: None,
        is_flags: false,
    }];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&[], &ENUMS, &[], &[], &[], &[]);

    let out = haphe::generate(&WitGenerator::new("haphe:demo"), &REGISTRY)
        .expect("dyn enum method registries generate with the feature on");
    let text = String::from_utf8_lossy(&out.files[0].content);
    assert!(
        text.contains(
            "toggle-echo-dyn: func(this: toggle, value: toggle-echo-dyn-value) -> toggle-echo-dyn-value;"
        ),
        "got:\n{text}"
    );
}

// ── Foreign dispatch modes (erased `dyn` imports)

/// A generic FOREIGN function template with `&self` receiver, dyn-dispatched.
const fn dyn_foreign_fn() -> FunctionDescriptor<'static> {
    FunctionDescriptor {
        receiver: Some(Receiver::Ref),
        ..generic_module_fn(haphe::Dispatch::Dyn)
    }
}

/// Without the feature, `dyn` FOREIGN functions are rejected by the
/// capability check like provided ones — never silently routed.
#[test]
#[cfg(not(feature = "dyn-generics"))]
fn dyn_generic_foreign_fn_rejected_by_capabilities() {
    static FNS: [FunctionDescriptor; 1] = [dyn_foreign_fn()];
    static FOREIGN: [haphe::ForeignInterfaceDescriptor; 1] =
        [foreign_iface("test::Hooks", "Hooks", &[], &FNS)];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&[], &[], &[], &[], &[], &FOREIGN);

    match haphe::generate(&WitGenerator::new("haphe:demo"), &REGISTRY) {
        Err(haphe::GenerateError::Incompatible(errors)) => {
            assert!(
                errors.iter().any(|e| matches!(
                    e,
                    haphe::CompatibilityError::DynGenericsUnsupported { function: "echo" }
                )),
                "expected DynGenericsUnsupported, got: {errors:?}"
            );
        }
        other => panic!("expected Incompatible, got: {other:?}"),
    }
}

/// A `dyn` FOREIGN function declares ERASED addressing: exactly ONE import
/// under the PLAIN name whose generic slots are the shared case variants —
/// no monomorph imports (they would demand guest exports that don't exist),
/// and no `-dyn` suffix (there is no static sibling to distinguish from).
#[test]
#[cfg(feature = "dyn-generics")]
fn dyn_foreign_fn_emits_one_erased_import() {
    static INSTS: [&[TypeDescriptor]; 2] = [
        &[TypeDescriptor::Primitive(PrimitiveType::I64)],
        &[TypeDescriptor::String],
    ];
    static FNS: [FunctionDescriptor; 1] = [FunctionDescriptor {
        instantiations: &INSTS,
        ..dyn_foreign_fn()
    }];
    static FOREIGN: [haphe::ForeignInterfaceDescriptor; 1] =
        [foreign_iface("test::Hooks", "Hooks", &[], &FNS)];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&[], &[], &[], &[], &[], &FOREIGN);

    let out = haphe::generate(&WitGenerator::new("haphe:demo"), &REGISTRY)
        .expect("dyn foreign registries generate with the feature on");
    let text = String::from_utf8_lossy(&out.files[0].content);
    assert!(text.contains("variant echo-dyn-value {"), "got:\n{text}");
    assert!(text.contains("%s64(s64),"), "got:\n{text}");
    assert!(text.contains("string(string),"), "got:\n{text}");
    assert!(
        text.contains("/// haphe:dyn-foreign = echo"),
        "got:\n{text}"
    );
    assert!(
        text.contains("echo: func(value: echo-dyn-value) -> echo-dyn-value;"),
        "got:\n{text}"
    );
    assert!(!text.contains("echo-s64"), "no monomorphs — got:\n{text}");
    assert!(!text.contains("echo-dyn:"), "plain name — got:\n{text}");
}
