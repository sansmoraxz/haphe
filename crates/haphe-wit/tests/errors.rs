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
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[], &[]);

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
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[], &[]);

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
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[], &[]);

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
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[], &[]);

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
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[], &[]);

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
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[], &[]);

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
        is_flags: false,
    }];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&[], &ENUMS, &[], &[], &[]);

    let err = generate_err(&REGISTRY);
    assert!(
        matches!(err, WitGenError::InvalidWit { .. }),
        "got: {err:?}"
    );
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
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[], &[]);

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
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[], &[]);

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
        },
        haphe::ModuleDescriptor {
            name: "beta",
            doc: None,
            functions: &[],
            type_ids: &[TypeId::new("test::R2")],
            submodules: &[],
            constants: &[],
        },
    ];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &MODULES, &[]);

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
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[], &[]);

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
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[], &INSTANTIATIONS);

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
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[], &INSTANTIATIONS);

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
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[], &INSTANTIATIONS);

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
