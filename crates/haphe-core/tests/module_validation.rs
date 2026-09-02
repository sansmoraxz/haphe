//! Registry validation and capability checks must cover modules, duplicate
//! ids, and exposed-name collisions — not just top-level type bodies.

use haphe_core::{
    BackendCapabilities, ConstantDescriptor, FieldDescriptor, FunctionDescriptor, ModuleDescriptor,
    Ownership, PrimitiveType, RegistryError, StructDescriptor, ThreadSafety, TypeDescriptor,
    TypeId, TypeRegistry,
};

const I32: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::I32);
const GHOST_REF: TypeDescriptor<'static> = TypeDescriptor::Ref(TypeId::new("Ghost"));

const fn plain_struct(id: &'static str) -> StructDescriptor<'static> {
    StructDescriptor {
        id: TypeId::new(id),
        name: id,
        doc: None,
        fields: &[],
        methods: &[],
        constructors: &[],
        properties: &[],
        trait_impls: &[],
        thread_safety: ThreadSafety::NONE,
        generic_params: &[],
    }
}

const fn module_fn(
    name: &'static str,
    ty: &'static TypeDescriptor<'static>,
    is_async: bool,
) -> FunctionDescriptor<'static> {
    FunctionDescriptor {
        name,
        doc: None,
        receiver: None,
        params: &[],
        return_type: ty,
        return_ownership: Ownership::Owned,
        is_async,
        error_kind: None,
    }
}

#[test]
fn duplicate_type_ids_fail_validation() {
    static STRUCTS: [StructDescriptor<'static>; 2] = [plain_struct("Point"), plain_struct("Point")];
    static REGISTRY: TypeRegistry<'static> = TypeRegistry::new(&STRUCTS, &[], &[], &[]);
    let errors = REGISTRY.validate().unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, RegistryError::DuplicateType { .. })),
        "{errors:?}"
    );
}

#[test]
fn dangling_ref_in_module_fn_fails_validation() {
    static FNS: [FunctionDescriptor<'static>; 1] = [module_fn("use_ghost", &GHOST_REF, false)];
    static MODULES: [ModuleDescriptor<'static>; 1] = [ModuleDescriptor {
        name: "m",
        doc: None,
        functions: &FNS,
        type_ids: &[],
        submodules: &[],
        constants: &[],
    }];
    static REGISTRY: TypeRegistry<'static> = TypeRegistry::new(&[], &[], &[], &MODULES);
    let errors = REGISTRY.validate().unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, RegistryError::DanglingModuleRef { module: "m", .. })),
        "{errors:?}"
    );
}

#[test]
fn dangling_type_id_in_nested_module_fails_validation() {
    static INNER: [ModuleDescriptor<'static>; 1] = [ModuleDescriptor {
        name: "inner",
        doc: None,
        functions: &[],
        type_ids: &[TypeId::new("Ghost")],
        submodules: &[],
        constants: &[],
    }];
    static MODULES: [ModuleDescriptor<'static>; 1] = [ModuleDescriptor {
        name: "outer",
        doc: None,
        functions: &[],
        type_ids: &[],
        submodules: &INNER,
        constants: &[],
    }];
    static REGISTRY: TypeRegistry<'static> = TypeRegistry::new(&[], &[], &[], &MODULES);
    let errors = REGISTRY.validate().unwrap_err();
    assert!(
        errors.iter().any(|e| matches!(
            e,
            RegistryError::DanglingModuleRef {
                module: "inner",
                ..
            }
        )),
        "{errors:?}"
    );
}

#[test]
fn duplicate_member_names_fail_validation() {
    static FIELDS: [FieldDescriptor<'static>; 1] = [FieldDescriptor {
        name: "x",
        doc: None,
        ty: &I32,
        readonly: false,
    }];
    static METHODS: [FunctionDescriptor<'static>; 1] = [module_fn("x", &I32, false)];
    static STRUCTS: [StructDescriptor<'static>; 1] = [StructDescriptor {
        fields: &FIELDS,
        methods: &METHODS,
        ..plain_struct("Point")
    }];
    static REGISTRY: TypeRegistry<'static> = TypeRegistry::new(&STRUCTS, &[], &[], &[]);
    let errors = REGISTRY.validate().unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, RegistryError::DuplicateMember { name: "x", .. })),
        "{errors:?}"
    );
}

#[test]
fn duplicate_module_entries_fail_validation() {
    static FNS: [FunctionDescriptor<'static>; 1] = [module_fn("pi", &I32, false)];
    static CONSTANTS: [ConstantDescriptor<'static>; 1] = [ConstantDescriptor {
        name: "pi",
        doc: None,
        ty: &I32,
        value: "3",
    }];
    static MODULES: [ModuleDescriptor<'static>; 1] = [ModuleDescriptor {
        name: "m",
        doc: None,
        functions: &FNS,
        type_ids: &[],
        submodules: &[],
        constants: &CONSTANTS,
    }];
    static REGISTRY: TypeRegistry<'static> = TypeRegistry::new(&[], &[], &[], &MODULES);
    let errors = REGISTRY.validate().unwrap_err();
    assert!(
        errors.iter().any(|e| matches!(
            e,
            RegistryError::DuplicateModuleEntry {
                module: "m",
                name: "pi"
            }
        )),
        "{errors:?}"
    );
}

#[test]
fn capability_check_covers_module_fns() {
    static FNS: [FunctionDescriptor<'static>; 1] = [module_fn("fetch", &I32, true)];
    static MODULES: [ModuleDescriptor<'static>; 1] = [ModuleDescriptor {
        name: "net",
        doc: None,
        functions: &FNS,
        type_ids: &[],
        submodules: &[],
        constants: &[],
    }];
    static REGISTRY: TypeRegistry<'static> = TypeRegistry::new(&[], &[], &[], &MODULES);
    let validated = REGISTRY.validate().unwrap();
    let errors = BackendCapabilities::ALL
        .with_async_fns(false)
        .check(&validated)
        .unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:?}");
}

#[test]
fn capability_check_covers_property_callbacks() {
    use haphe_core::PropertyDescriptor;
    const CALLBACK: TypeDescriptor<'static> = TypeDescriptor::Callback {
        params: &[],
        return_type: &I32,
    };
    static PROPS: [PropertyDescriptor<'static>; 1] = [PropertyDescriptor {
        name: "hook",
        doc: None,
        ty: &CALLBACK,
        readonly: true,
    }];
    static STRUCTS: [StructDescriptor<'static>; 1] = [StructDescriptor {
        properties: &PROPS,
        ..plain_struct("Hooked")
    }];
    static REGISTRY: TypeRegistry<'static> = TypeRegistry::new(&STRUCTS, &[], &[], &[]);
    let validated = REGISTRY.validate().unwrap();
    let errors = BackendCapabilities::ALL
        .with_callbacks(false)
        .check(&validated)
        .unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:?}");
}

#[test]
fn const_eq_matches_partial_eq() {
    const A: TypeDescriptor<'static> = TypeDescriptor::Option(&TypeDescriptor::List(&I32));
    const B: TypeDescriptor<'static> = TypeDescriptor::Option(&TypeDescriptor::List(&I32));
    const C: TypeDescriptor<'static> = TypeDescriptor::Option(&TypeDescriptor::String);
    const { assert!(A.const_eq(&B)) };
    const { assert!(!A.const_eq(&C)) };
    const { assert!(GHOST_REF.const_eq(&TypeDescriptor::Ref(TypeId::new("Ghost")))) };
    const { assert!(!GHOST_REF.const_eq(&TypeDescriptor::Ref(TypeId::new("Other")))) };
    assert_eq!(A == B, A.const_eq(&B));
}
