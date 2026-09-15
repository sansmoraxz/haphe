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
