//! Registry validation and capability checks for foreign interfaces.

use haphe_core::{
    BackendCapabilities, CompatibilityError, ForeignInterfaceDescriptor, FunctionDescriptor,
    Ownership, ParamDescriptor, PrimitiveType, Receiver, RegistryError, StructDescriptor,
    ThreadSafety, TypeDescriptor, TypeId, TypeRegistry, TypeRegistryBuilder,
};

static I32_TYPE: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::I32);
static UNIT_TYPE: TypeDescriptor<'static> = TypeDescriptor::Unit;

const fn method(
    name: &'static str,
    params: &'static [ParamDescriptor<'static>],
    return_type: &'static TypeDescriptor<'static>,
    is_async: bool,
) -> FunctionDescriptor<'static> {
    FunctionDescriptor {
        name,
        doc: None,
        receiver: Some(Receiver::Ref),
        generic_params: &[],
        instantiations: &[],
        params,
        return_type,
        return_ownership: Ownership::Owned,
        is_async,
        error_kind: None,
    }
}

static LOG_PARAMS: [ParamDescriptor<'static>; 1] = [ParamDescriptor {
    name: "message",
    ty: &TypeDescriptor::String,
    ownership: Ownership::Owned,
}];

static HOOKS_FNS: [FunctionDescriptor<'static>; 2] = [
    method("log", &LOG_PARAMS, &UNIT_TYPE, false),
    method("poll", &[], &I32_TYPE, true),
];

static HOOKS: ForeignInterfaceDescriptor<'static> = ForeignInterfaceDescriptor {
    id: TypeId::new("HostHooks"),
    name: "HostHooks",
    doc: None,
    generic_params: &[],
    functions: &HOOKS_FNS,
    thread_safety: ThreadSafety::NONE,
};

static REGISTRY: TypeRegistry<'static> = TypeRegistry::new(&[], &[], &[], &[], &[], &FOREIGN);
static FOREIGN: [ForeignInterfaceDescriptor<'static>; 1] = [HOOKS];

#[test]
fn valid_foreign_interface_passes() {
    let validated = REGISTRY.validate().expect("registry should validate");
    let fi = validated
        .get_foreign_interface(&TypeId::new("HostHooks"))
        .expect("interface is registered");
    assert_eq!(fi.functions.len(), 2);
    assert_eq!(validated.foreign_interfaces().len(), 1);
}

#[test]
fn dangling_ref_in_foreign_signature_is_rejected() {
    static BAD_RET: TypeDescriptor<'static> = TypeDescriptor::Ref(TypeId::new("Missing"));
    static FNS: [FunctionDescriptor<'static>; 1] = [method("get", &[], &BAD_RET, false)];
    static FOREIGN: [ForeignInterfaceDescriptor<'static>; 1] = [ForeignInterfaceDescriptor {
        id: TypeId::new("Bad"),
        name: "Bad",
        doc: None,
        generic_params: &[],
        functions: &FNS,
        thread_safety: ThreadSafety::NONE,
    }];
    static REGISTRY: TypeRegistry<'static> = TypeRegistry::new(&[], &[], &[], &[], &[], &FOREIGN);
    let errors = REGISTRY.validate().unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        RegistryError::DanglingRef { from, to }
            if *from == TypeId::new("Bad") && *to == TypeId::new("Missing")
    )));
}

#[test]
fn ref_to_foreign_interface_is_dangling() {
    // Foreign interfaces are not value types: a struct field cannot
    // reference one.
    static FIELD_TY: TypeDescriptor<'static> = TypeDescriptor::Ref(TypeId::new("HostHooks"));
    static FIELDS: [haphe_core::FieldDescriptor<'static>; 1] = [haphe_core::FieldDescriptor {
        name: "hooks",
        doc: None,
        ty: &FIELD_TY,
        readonly: false,
    }];
    static STRUCTS: [StructDescriptor<'static>; 1] = [StructDescriptor {
        id: TypeId::new("Holder"),
        name: "Holder",
        doc: None,
        fields: &FIELDS,
        methods: &[],
        constructors: &[],
        properties: &[],
        trait_impls: &[],
        thread_safety: ThreadSafety::NONE,
        generic_params: &[],
    }];
    static REGISTRY: TypeRegistry<'static> =
        TypeRegistry::new(&STRUCTS, &[], &[], &[], &[], &FOREIGN);
    let errors = REGISTRY.validate().unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, RegistryError::DanglingRef { .. }))
    );
}

#[test]
fn foreign_id_colliding_with_struct_is_duplicate() {
    static STRUCTS: [StructDescriptor<'static>; 1] = [StructDescriptor {
        id: TypeId::new("HostHooks"),
        name: "HostHooks",
        doc: None,
        fields: &[],
        methods: &[],
        constructors: &[],
        properties: &[],
        trait_impls: &[],
        thread_safety: ThreadSafety::NONE,
        generic_params: &[],
    }];
    static REGISTRY: TypeRegistry<'static> =
        TypeRegistry::new(&STRUCTS, &[], &[], &[], &[], &FOREIGN);
    let errors = REGISTRY.validate().unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        RegistryError::DuplicateType { id } if *id == TypeId::new("HostHooks")
    )));
}

#[test]
fn duplicate_method_names_are_rejected() {
    static FNS: [FunctionDescriptor<'static>; 2] = [
        method("go", &[], &UNIT_TYPE, false),
        method("go", &[], &I32_TYPE, false),
    ];
    static FOREIGN: [ForeignInterfaceDescriptor<'static>; 1] = [ForeignInterfaceDescriptor {
        id: TypeId::new("Dup"),
        name: "Dup",
        doc: None,
        generic_params: &[],
        functions: &FNS,
        thread_safety: ThreadSafety::NONE,
    }];
    static REGISTRY: TypeRegistry<'static> = TypeRegistry::new(&[], &[], &[], &[], &[], &FOREIGN);
    let errors = REGISTRY.validate().unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        RegistryError::DuplicateMember { owner, name }
            if *owner == TypeId::new("Dup") && *name == "go"
    )));
}

// ── Generic foreign interfaces

static T_PARAM: [haphe_core::GenericParam<'static>; 1] = [haphe_core::GenericParam {
    name: "T",
    bounds: &[],
    default: None,
}];
static T_TYPE: TypeDescriptor<'static> = TypeDescriptor::GenericParam("T");
static STORE_FNS: [FunctionDescriptor<'static>; 1] = [method("get", &[], &T_TYPE, false)];
static STORE: ForeignInterfaceDescriptor<'static> = ForeignInterfaceDescriptor {
    id: TypeId::new("Store"),
    name: "Store",
    doc: None,
    generic_params: &T_PARAM,
    functions: &STORE_FNS,
    thread_safety: ThreadSafety::NONE,
};
static STORE_FOREIGN: [ForeignInterfaceDescriptor<'static>; 1] = [STORE];

#[test]
fn declared_generic_param_in_foreign_signature_validates() {
    static INSTS: [haphe_core::InstantiationDescriptor<'static>; 1] =
        [haphe_core::InstantiationDescriptor {
            id: TypeId::new("Store"),
            args: &[I32_TYPE],
        }];
    static REGISTRY: TypeRegistry<'static> =
        TypeRegistry::new(&[], &[], &[], &[], &INSTS, &STORE_FOREIGN);
    let validated = REGISTRY.validate().expect("declared param validates");
    BackendCapabilities::ALL
        .check(&validated)
        .expect("instantiated generic foreign interface is compatible");
}

#[test]
fn undeclared_generic_param_in_foreign_signature_is_rejected() {
    static FNS: [FunctionDescriptor<'static>; 1] = [method(
        "get",
        &[],
        &TypeDescriptor::GenericParam("U"),
        false,
    )];
    static FOREIGN: [ForeignInterfaceDescriptor<'static>; 1] = [ForeignInterfaceDescriptor {
        id: TypeId::new("Bad"),
        name: "Bad",
        doc: None,
        generic_params: &T_PARAM,
        functions: &FNS,
        thread_safety: ThreadSafety::NONE,
    }];
    static REGISTRY: TypeRegistry<'static> = TypeRegistry::new(&[], &[], &[], &[], &[], &FOREIGN);
    let errors = REGISTRY.validate().unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        RegistryError::UndeclaredGenericParam { owner, param_name }
            if *owner == TypeId::new("Bad") && *param_name == "U"
    )));
}

#[test]
fn foreign_instantiation_arity_and_concreteness_are_checked() {
    static BAD_INSTS: [haphe_core::InstantiationDescriptor<'static>; 2] = [
        haphe_core::InstantiationDescriptor {
            id: TypeId::new("Store"),
            args: &[I32_TYPE, UNIT_TYPE],
        },
        haphe_core::InstantiationDescriptor {
            id: TypeId::new("Store"),
            args: &[T_TYPE],
        },
    ];
    static REGISTRY: TypeRegistry<'static> =
        TypeRegistry::new(&[], &[], &[], &[], &BAD_INSTS, &STORE_FOREIGN);
    let errors = REGISTRY.validate().unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        RegistryError::InstantiationArityMismatch { target, expected: 1, found: 2 }
            if *target == TypeId::new("Store")
    )));
    assert!(errors.iter().any(|e| matches!(
        e,
        RegistryError::NonConcreteInstantiation { target, param_name: "T" }
            if *target == TypeId::new("Store")
    )));
}

#[test]
fn uninstantiated_generic_foreign_interface_is_rejected() {
    static REGISTRY: TypeRegistry<'static> =
        TypeRegistry::new(&[], &[], &[], &[], &[], &STORE_FOREIGN);
    let validated = REGISTRY.validate().unwrap();
    let errors = BackendCapabilities::ALL.check(&validated).unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        CompatibilityError::UninstantiatedGeneric { type_id }
            if *type_id == TypeId::new("Store")
    )));

    let errors = BackendCapabilities::ALL
        .with_generics(false)
        .check(&validated)
        .unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        CompatibilityError::UnsupportedGenerics { type_id }
            if *type_id == TypeId::new("Store")
    )));

    // The foreign gate stays primary: without foreign_fns only the
    // interface-level error is reported.
    let errors = BackendCapabilities::ALL
        .with_foreign_fns(false)
        .check(&validated)
        .unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        errors[0],
        CompatibilityError::UnsupportedForeignInterface { .. }
    ));
}

#[test]
fn builder_handles_generic_foreign_instantiations() {
    static ARGS: [TypeDescriptor<'static>; 1] = [I32_TYPE];
    let mut builder = TypeRegistryBuilder::new();
    builder.register_foreign_interface(STORE).unwrap();
    builder.register_instantiation(haphe_core::InstantiationDescriptor {
        id: TypeId::new("Store"),
        args: &ARGS,
    });
    builder.register_instantiation(haphe_core::InstantiationDescriptor {
        id: TypeId::new("Missing"),
        args: &ARGS,
    });
    let registry = builder.as_registry();
    let errors = registry.validate().unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        RegistryError::DanglingInstantiation { to } if *to == TypeId::new("Missing")
    )));
}

// ── Function-level generics

const fn generic_fn(
    name: &'static str,
    generic_params: &'static [haphe_core::GenericParam<'static>],
    instantiations: &'static [&'static [TypeDescriptor<'static>]],
    return_type: &'static TypeDescriptor<'static>,
) -> FunctionDescriptor<'static> {
    FunctionDescriptor {
        name,
        doc: None,
        receiver: Some(Receiver::Ref),
        generic_params,
        instantiations,
        params: &[],
        return_type,
        return_ownership: Ownership::Owned,
        is_async: false,
        error_kind: None,
    }
}

static U_PARAM: [haphe_core::GenericParam<'static>; 1] = [haphe_core::GenericParam {
    name: "U",
    bounds: &[],
    default: None,
}];
static U_TYPE: TypeDescriptor<'static> = TypeDescriptor::GenericParam("U");
static I32_ARGS: [TypeDescriptor<'static>; 1] = [I32_TYPE];

#[test]
fn function_instantiations_validate() {
    static INSTS: [&[TypeDescriptor<'static>]; 1] = [&I32_ARGS];
    static FNS: [FunctionDescriptor<'static>; 1] =
        [generic_fn("convert", &U_PARAM, &INSTS, &U_TYPE)];
    static FOREIGN: [ForeignInterfaceDescriptor<'static>; 1] = [ForeignInterfaceDescriptor {
        id: TypeId::new("Conv"),
        name: "Conv",
        doc: None,
        generic_params: &[],
        functions: &FNS,
        thread_safety: ThreadSafety::NONE,
    }];
    static REGISTRY: TypeRegistry<'static> = TypeRegistry::new(&[], &[], &[], &[], &[], &FOREIGN);
    let validated = REGISTRY.validate().expect("valid instantiations");
    BackendCapabilities::ALL
        .check(&validated)
        .expect("instantiated generic function is compatible");
    let errors = BackendCapabilities::ALL
        .with_generics(false)
        .check(&validated)
        .unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        CompatibilityError::UnsupportedGenerics { type_id } if *type_id == TypeId::new("Conv")
    )));
}

#[test]
fn bad_function_instantiations_are_rejected() {
    static TWO_ARGS: [TypeDescriptor<'static>; 2] = [I32_TYPE, UNIT_TYPE];
    static NON_CONCRETE: [TypeDescriptor<'static>; 1] = [U_TYPE];
    static BAD_INSTS: [&[TypeDescriptor<'static>]; 2] = [&TWO_ARGS, &NON_CONCRETE];
    static ON_NON_GENERIC: [&[TypeDescriptor<'static>]; 1] = [&I32_ARGS];
    static FNS: [FunctionDescriptor<'static>; 2] = [
        generic_fn("convert", &U_PARAM, &BAD_INSTS, &U_TYPE),
        generic_fn("plain", &[], &ON_NON_GENERIC, &UNIT_TYPE),
    ];
    static FOREIGN: [ForeignInterfaceDescriptor<'static>; 1] = [ForeignInterfaceDescriptor {
        id: TypeId::new("Bad"),
        name: "Bad",
        doc: None,
        generic_params: &[],
        functions: &FNS,
        thread_safety: ThreadSafety::NONE,
    }];
    static REGISTRY: TypeRegistry<'static> = TypeRegistry::new(&[], &[], &[], &[], &[], &FOREIGN);
    let errors = REGISTRY.validate().unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        RegistryError::FunctionInstantiationArityMismatch {
            function: "convert",
            expected: 1,
            found: 2,
        }
    )));
    assert!(errors.iter().any(|e| matches!(
        e,
        RegistryError::NonConcreteFunctionInstantiation {
            function: "convert",
            param_name: "U",
        }
    )));
    assert!(errors.iter().any(|e| matches!(
        e,
        RegistryError::InstantiationOnNonGenericFunction { function: "plain" }
    )));
}

#[test]
fn uninstantiated_generic_function_is_rejected() {
    static FNS: [FunctionDescriptor<'static>; 1] = [generic_fn("convert", &U_PARAM, &[], &U_TYPE)];
    static FOREIGN: [ForeignInterfaceDescriptor<'static>; 1] = [ForeignInterfaceDescriptor {
        id: TypeId::new("Conv"),
        name: "Conv",
        doc: None,
        generic_params: &[],
        functions: &FNS,
        thread_safety: ThreadSafety::NONE,
    }];
    static REGISTRY: TypeRegistry<'static> = TypeRegistry::new(&[], &[], &[], &[], &[], &FOREIGN);
    let validated = REGISTRY.validate().unwrap();
    let errors = BackendCapabilities::ALL.check(&validated).unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        CompatibilityError::UninstantiatedGeneric { type_id } if *type_id == TypeId::new("Conv")
    )));
}

#[test]
fn builder_registers_foreign_interfaces() {
    let mut builder = TypeRegistryBuilder::new();
    builder.register_foreign_interface(HOOKS).unwrap();
    assert!(matches!(
        builder.register_foreign_interface(HOOKS),
        Err(RegistryError::DuplicateType { .. })
    ));
    let registry = builder.as_registry();
    assert_eq!(registry.foreign_interfaces().len(), 1);
    registry.validate().expect("builder registry validates");
}

#[test]
fn backend_without_foreign_support_rejects() {
    let validated = REGISTRY.validate().unwrap();
    let caps = BackendCapabilities::ALL.with_foreign_fns(false);
    let errors = caps.check(&validated).unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        CompatibilityError::UnsupportedForeignInterface { type_id }
            if *type_id == TypeId::new("HostHooks")
    )));
}

#[test]
fn backend_without_async_rejects_async_foreign_fn() {
    let validated = REGISTRY.validate().unwrap();
    let caps = BackendCapabilities::ALL.with_async_fns(false);
    let errors = caps.check(&validated).unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        CompatibilityError::UnsupportedAsync { type_id, fn_name }
            if *type_id == TypeId::new("HostHooks") && *fn_name == "poll"
    )));
}

#[test]
fn required_thread_safety_applies_to_foreign_interfaces() {
    let validated = REGISTRY.validate().unwrap();
    let caps = BackendCapabilities::ALL.with_required_thread_safety(Some(ThreadSafety::SEND));
    let errors = caps.check(&validated).unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        CompatibilityError::InsufficientThreadSafety { type_id, .. }
            if *type_id == TypeId::new("HostHooks")
    )));
}
