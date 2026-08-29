use haphe_core::{
    BackendCapabilities, CompatibilityError, ConstantDescriptor, Describe, EnumDescriptor,
    EnumVariant, FieldDescriptor, FunctionDescriptor, GenericParam, ModuleDescriptor, Ownership,
    ParamDescriptor, PrimitiveType, PropertyDescriptor, Receiver, RegistryError, StructDescriptor,
    ThreadSafety, TraitImpl, TypeAliasDescriptor, TypeDescriptor, TypeId, TypeKind, TypeRegistry,
    TypeRegistryBuilder, VariantKind,
};

// ── Static type descriptors

static F64_TYPE: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::F64);
static BOOL_TYPE: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::Bool);
static I32_TYPE: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::I32);
static POINT_REF: TypeDescriptor<'static> = TypeDescriptor::Ref(TypeId::new("Point"));
static STRING_TYPE: TypeDescriptor<'static> = TypeDescriptor::String;

// ── Const-constructed descriptors (compile-time test)

static POINT_PARAMS: [ParamDescriptor<'static>; 1] = [ParamDescriptor {
    name: "other",
    ty: &POINT_REF,
    ownership: Ownership::Ref,
}];

static POINT_METHODS: [FunctionDescriptor<'static>; 1] = [FunctionDescriptor {
    name: "distance_to",
    doc: None,
    receiver: Some(Receiver::Ref),
    params: &POINT_PARAMS,
    return_type: &F64_TYPE,
    return_ownership: Ownership::Owned,
    is_async: false,
    error_kind: None,
}];

static POINT_FIELDS: [FieldDescriptor<'static>; 2] = [
    FieldDescriptor {
        name: "x",
        doc: None,
        ty: &F64_TYPE,
        readonly: false,
    },
    FieldDescriptor {
        name: "y",
        doc: None,
        ty: &F64_TYPE,
        readonly: false,
    },
];

static POINT_DESC: StructDescriptor<'static> = StructDescriptor {
    id: TypeId::new("Point"),
    name: "Point",
    doc: Some("A 2D point"),
    fields: &POINT_FIELDS,
    methods: &POINT_METHODS,
    constructors: &[],
    properties: &[],
    trait_impls: &[],
    thread_safety: ThreadSafety::SEND_SYNC,
    generic_params: &[],
};

static RGB_FIELDS: [TypeDescriptor<'static>; 3] = [
    TypeDescriptor::Primitive(PrimitiveType::U8),
    TypeDescriptor::Primitive(PrimitiveType::U8),
    TypeDescriptor::Primitive(PrimitiveType::U8),
];

static NAMED_FIELDS: [FieldDescriptor<'static>; 1] = [FieldDescriptor {
    name: "name",
    doc: None,
    ty: &STRING_TYPE,
    readonly: true,
}];

static COLOR_VARIANTS: [EnumVariant<'static>; 3] = [
    EnumVariant {
        name: "Red",
        doc: None,
        kind: VariantKind::Unit,
    },
    EnumVariant {
        name: "Rgb",
        doc: None,
        kind: VariantKind::Tuple(&RGB_FIELDS),
    },
    EnumVariant {
        name: "Named",
        doc: None,
        kind: VariantKind::Struct(&NAMED_FIELDS),
    },
];

static COLOR_DESC: EnumDescriptor<'static> = EnumDescriptor {
    id: TypeId::new("Color"),
    name: "Color",
    doc: None,
    variants: &COLOR_VARIANTS,
    methods: &[],
    trait_impls: &[],
    thread_safety: ThreadSafety::SEND_SYNC,
    generic_params: &[],
};

// ── Describe impls (builder path)

struct Point;

impl Describe for Point {
    fn describe(builder: &mut TypeRegistryBuilder<'static>) {
        builder.register_struct(POINT_DESC).unwrap();
    }
}

struct Color;

impl Describe for Color {
    fn describe(builder: &mut TypeRegistryBuilder<'static>) {
        builder.register_enum(COLOR_DESC).unwrap();
    }
}

// ── Tests

#[test]
fn const_constructed_registry() {
    static STRUCTS: [StructDescriptor<'static>; 1] = [POINT_DESC];
    static ENUMS: [EnumDescriptor<'static>; 1] = [COLOR_DESC];

    let registry = TypeRegistry::new(&STRUCTS, &ENUMS, &[], &[]);

    let point = registry.get_struct(&TypeId::new("Point")).unwrap();
    assert_eq!(point.name, "Point");
    assert_eq!(point.fields.len(), 2);

    let color = registry.get_enum(&TypeId::new("Color")).unwrap();
    assert_eq!(color.name, "Color");
    assert_eq!(color.variants.len(), 3);
}

#[test]
fn describe_registers_struct() {
    let mut builder = TypeRegistryBuilder::new();
    Point::describe(&mut builder);

    let registry = builder.as_registry();
    let point = registry.get_struct(&TypeId::new("Point")).unwrap();
    assert_eq!(point.name, "Point");
    assert_eq!(point.fields.len(), 2);
    assert_eq!(point.methods.len(), 1);
    assert_eq!(point.methods[0].name, "distance_to");
}

#[test]
fn describe_registers_enum() {
    let mut builder = TypeRegistryBuilder::new();
    Color::describe(&mut builder);

    let registry = builder.as_registry();
    let color = registry.get_enum(&TypeId::new("Color")).unwrap();
    assert_eq!(color.name, "Color");
    assert_eq!(color.variants.len(), 3);

    assert!(matches!(color.variants[0].kind, VariantKind::Unit));
    assert!(matches!(color.variants[1].kind, VariantKind::Tuple(v) if v.len() == 3));
    assert!(matches!(color.variants[2].kind, VariantKind::Struct(f) if f.len() == 1));
}

#[test]
fn registry_lookup_missing_returns_none() {
    let registry = TypeRegistry::new(&[], &[], &[], &[]);
    assert!(registry.get_struct(&TypeId::new("Missing")).is_none());
    assert!(registry.get_enum(&TypeId::new("Missing")).is_none());
}

#[test]
fn registry_iterators() {
    let mut builder = TypeRegistryBuilder::new();
    Point::describe(&mut builder);
    Color::describe(&mut builder);

    let registry = builder.as_registry();
    assert_eq!(registry.structs().len(), 1);
    assert_eq!(registry.enums().len(), 1);
}

#[test]
fn nested_type_descriptors() {
    static INNER_LIST: TypeDescriptor<'static> = TypeDescriptor::List(&POINT_REF);
    static OUTER: TypeDescriptor<'static> = TypeDescriptor::Option(&INNER_LIST);

    match &OUTER {
        TypeDescriptor::Option(inner) => match inner {
            TypeDescriptor::List(elem) => match elem {
                TypeDescriptor::Ref(id) => assert_eq!(id.as_str(), "Point"),
                other => panic!("expected Ref, got {other:?}"),
            },
            other => panic!("expected List, got {other:?}"),
        },
        other => panic!("expected Option, got {other:?}"),
    }
}

#[test]
fn module_tree() {
    static ORIGIN_RET: TypeDescriptor<'static> = TypeDescriptor::Ref(TypeId::new("Point"));
    static ORIGIN_FN: [FunctionDescriptor<'static>; 1] = [FunctionDescriptor {
        name: "origin",
        doc: None,
        receiver: None,
        params: &[],
        return_type: &ORIGIN_RET,
        return_ownership: Ownership::Owned,
        is_async: false,
        error_kind: None,
    }];
    static PI_TYPE: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::F64);
    static CONSTS: [ConstantDescriptor<'static>; 1] = [ConstantDescriptor {
        name: "PI",
        doc: None,
        ty: &PI_TYPE,
        value: "3.14159265358979",
    }];
    static SHAPES: [ModuleDescriptor<'static>; 1] = [ModuleDescriptor {
        name: "shapes",
        doc: None,
        functions: &[],
        type_ids: &[],
        submodules: &[],
        constants: &[],
    }];
    static TYPE_IDS: [TypeId<'static>; 2] = [TypeId::new("Point"), TypeId::new("Color")];
    static MODULES: [ModuleDescriptor<'static>; 1] = [ModuleDescriptor {
        name: "geometry",
        doc: Some("Geometry utilities"),
        functions: &ORIGIN_FN,
        type_ids: &TYPE_IDS,
        submodules: &SHAPES,
        constants: &CONSTS,
    }];

    let registry = TypeRegistry::new(&[], &[], &[], &MODULES);
    assert_eq!(registry.modules().len(), 1);
    let m = &registry.modules()[0];
    assert_eq!(m.name, "geometry");
    assert_eq!(m.functions.len(), 1);
    assert_eq!(m.type_ids.len(), 2);
    assert_eq!(m.submodules.len(), 1);
    assert_eq!(m.constants.len(), 1);
    assert_eq!(m.constants[0].name, "PI");
}

#[test]
fn duplicate_struct_registration_errors() {
    let mut builder = TypeRegistryBuilder::new();
    Point::describe(&mut builder);

    let result = builder.register_struct(StructDescriptor {
        id: TypeId::new("Point"),
        name: "Point",
        doc: None,
        fields: &[],
        methods: &[],
        constructors: &[],
        properties: &[],
        trait_impls: &[],
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &[],
    });
    assert!(matches!(result, Err(RegistryError::DuplicateType { .. })));
}

#[test]
fn duplicate_enum_registration_errors() {
    let mut builder = TypeRegistryBuilder::new();
    Color::describe(&mut builder);

    let result = builder.register_enum(EnumDescriptor {
        id: TypeId::new("Color"),
        name: "Color",
        doc: None,
        variants: &[],
        methods: &[],
        trait_impls: &[],
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &[],
    });
    assert!(matches!(result, Err(RegistryError::DuplicateType { .. })));
}

#[test]
fn cross_kind_duplicate_errors() {
    let mut builder = TypeRegistryBuilder::new();
    Point::describe(&mut builder);

    let result = builder.register_enum(EnumDescriptor {
        id: TypeId::new("Point"),
        name: "Point",
        doc: None,
        variants: &[],
        methods: &[],
        trait_impls: &[],
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &[],
    });
    assert!(matches!(result, Err(RegistryError::DuplicateType { .. })));
}

#[test]
fn get_type_unified_lookup() {
    let mut builder = TypeRegistryBuilder::new();
    Point::describe(&mut builder);
    Color::describe(&mut builder);

    let registry = builder.as_registry();
    assert!(matches!(
        registry.get_type(&TypeId::new("Point")),
        Some(TypeKind::Struct(_))
    ));
    assert!(matches!(
        registry.get_type(&TypeId::new("Color")),
        Some(TypeKind::Enum(_))
    ));
    assert!(registry.get_type(&TypeId::new("Missing")).is_none());
}

#[test]
fn validate_passes_for_valid_registry() {
    let mut builder = TypeRegistryBuilder::new();
    Point::describe(&mut builder);
    Color::describe(&mut builder);

    let registry = builder.as_registry();
    assert!(registry.validate().is_ok());
}

#[test]
fn validate_catches_dangling_ref_in_field() {
    static DANGLING_REF: TypeDescriptor<'static> = TypeDescriptor::Ref(TypeId::new("Missing"));
    static BAD_FIELDS: [FieldDescriptor<'static>; 1] = [FieldDescriptor {
        name: "bad",
        doc: None,
        ty: &DANGLING_REF,
        readonly: false,
    }];
    static BAD_STRUCT: [StructDescriptor<'static>; 1] = [StructDescriptor {
        id: TypeId::new("Bad"),
        name: "Bad",
        doc: None,
        fields: &BAD_FIELDS,
        methods: &[],
        constructors: &[],
        properties: &[],
        trait_impls: &[],
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &[],
    }];

    let registry = TypeRegistry::new(&BAD_STRUCT, &[], &[], &[]);
    let errors = registry.validate().unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        &errors[0],
        RegistryError::DanglingRef { from, to }
        if from.as_str() == "Bad" && to.as_str() == "Missing"
    ));
}

#[test]
fn validate_catches_dangling_ref_in_method_param() {
    static MISSING_REF: TypeDescriptor<'static> = TypeDescriptor::Ref(TypeId::new("Unknown"));
    static PARAMS: [ParamDescriptor<'static>; 1] = [ParamDescriptor {
        name: "x",
        ty: &MISSING_REF,
        ownership: Ownership::Owned,
    }];
    static UNIT: TypeDescriptor<'static> = TypeDescriptor::Unit;
    static METHODS: [FunctionDescriptor<'static>; 1] = [FunctionDescriptor {
        name: "bad_method",
        doc: None,
        receiver: Some(Receiver::Ref),
        params: &PARAMS,
        return_type: &UNIT,
        return_ownership: Ownership::Owned,
        is_async: false,
        error_kind: None,
    }];
    static STRUCTS: [StructDescriptor<'static>; 1] = [StructDescriptor {
        id: TypeId::new("HasBadMethod"),
        name: "HasBadMethod",
        doc: None,
        fields: &[],
        methods: &METHODS,
        constructors: &[],
        properties: &[],
        trait_impls: &[],
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &[],
    }];

    let registry = TypeRegistry::new(&STRUCTS, &[], &[], &[]);
    let errors = registry.validate().unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        &errors[0],
        RegistryError::DanglingRef { to, .. } if to.as_str() == "Unknown"
    ));
}

#[test]
fn validate_catches_nested_dangling_ref() {
    static DANGLING: TypeDescriptor<'static> = TypeDescriptor::Ref(TypeId::new("Deep"));
    static LIST_OF: TypeDescriptor<'static> = TypeDescriptor::List(&DANGLING);
    static OPT_LIST: TypeDescriptor<'static> = TypeDescriptor::Option(&LIST_OF);
    static FIELDS: [FieldDescriptor<'static>; 1] = [FieldDescriptor {
        name: "nested",
        doc: None,
        ty: &OPT_LIST,
        readonly: false,
    }];
    static STRUCTS: [StructDescriptor<'static>; 1] = [StructDescriptor {
        id: TypeId::new("Wrapper"),
        name: "Wrapper",
        doc: None,
        fields: &FIELDS,
        methods: &[],
        constructors: &[],
        properties: &[],
        trait_impls: &[],
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &[],
    }];

    let registry = TypeRegistry::new(&STRUCTS, &[], &[], &[]);
    let errors = registry.validate().unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        &errors[0],
        RegistryError::DanglingRef { to, .. } if to.as_str() == "Deep"
    ));
}

#[test]
fn validate_catches_dangling_ref_in_callback() {
    static MISSING: TypeDescriptor<'static> = TypeDescriptor::Ref(TypeId::new("Gone"));
    static CB_PARAMS: [TypeDescriptor<'static>; 1] =
        [TypeDescriptor::Primitive(PrimitiveType::I32)];
    static CB: TypeDescriptor<'static> = TypeDescriptor::Callback {
        params: &CB_PARAMS,
        return_type: &MISSING,
    };
    static FIELDS: [FieldDescriptor<'static>; 1] = [FieldDescriptor {
        name: "cb",
        doc: None,
        ty: &CB,
        readonly: false,
    }];
    static STRUCTS: [StructDescriptor<'static>; 1] = [StructDescriptor {
        id: TypeId::new("HasCb"),
        name: "HasCb",
        doc: None,
        fields: &FIELDS,
        methods: &[],
        constructors: &[],
        properties: &[],
        trait_impls: &[],
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &[],
    }];

    let registry = TypeRegistry::new(&STRUCTS, &[], &[], &[]);
    let errors = registry.validate().unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        &errors[0],
        RegistryError::DanglingRef { to, .. } if to.as_str() == "Gone"
    ));
}

#[test]
fn callback_type_descriptor() {
    static PARAMS: [TypeDescriptor<'static>; 2] = [
        TypeDescriptor::Primitive(PrimitiveType::I32),
        TypeDescriptor::String,
    ];
    static CB: TypeDescriptor<'static> = TypeDescriptor::Callback {
        params: &PARAMS,
        return_type: &BOOL_TYPE,
    };

    match &CB {
        TypeDescriptor::Callback {
            params,
            return_type,
        } => {
            assert_eq!(params.len(), 2);
            assert_eq!(*return_type, &BOOL_TYPE);
        }
        other => panic!("expected Callback, got {other:?}"),
    }
}

#[test]
fn array_type_descriptor() {
    static INNER: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::U8);
    static ARR: TypeDescriptor<'static> = TypeDescriptor::Array(&INNER, 32);

    match &ARR {
        TypeDescriptor::Array(inner, len) => {
            assert_eq!(**inner, TypeDescriptor::Primitive(PrimitiveType::U8));
            assert_eq!(*len, 32);
        }
        other => panic!("expected Array, got {other:?}"),
    }
}

#[test]
fn map_type_descriptor() {
    static MAP: TypeDescriptor<'static> = TypeDescriptor::Map(&STRING_TYPE, &I32_TYPE);

    match &MAP {
        TypeDescriptor::Map(k, v) => {
            assert_eq!(**k, TypeDescriptor::String);
            assert_eq!(**v, TypeDescriptor::Primitive(PrimitiveType::I32));
        }
        other => panic!("expected Map, got {other:?}"),
    }
}

#[test]
fn result_type_descriptor() {
    static RES: TypeDescriptor<'static> = TypeDescriptor::Result(&I32_TYPE, &STRING_TYPE);

    match &RES {
        TypeDescriptor::Result(ok, err) => {
            assert_eq!(**ok, TypeDescriptor::Primitive(PrimitiveType::I32));
            assert_eq!(**err, TypeDescriptor::String);
        }
        other => panic!("expected Result, got {other:?}"),
    }
}

#[test]
fn async_function_descriptor() {
    static RET: TypeDescriptor<'static> = TypeDescriptor::Unit;
    static FN_DESC: FunctionDescriptor<'static> = FunctionDescriptor {
        name: "fetch_data",
        doc: None,
        receiver: None,
        params: &[],
        return_type: &RET,
        return_ownership: Ownership::Owned,
        is_async: true,
        error_kind: None,
    };

    assert!(FN_DESC.is_async);
    assert_eq!(FN_DESC.name, "fetch_data");
}

#[test]
fn builder_to_registry_roundtrip() {
    let mut builder = TypeRegistryBuilder::new();
    Point::describe(&mut builder);
    Color::describe(&mut builder);

    let registry = builder.as_registry();
    assert!(registry.get_struct(&TypeId::new("Point")).is_some());
    assert!(registry.get_enum(&TypeId::new("Color")).is_some());
    assert!(registry.validate().is_ok());
}

// ── IR extensions

#[test]
fn constructors_on_struct() {
    static NEW_PARAMS: [ParamDescriptor<'static>; 2] = [
        ParamDescriptor {
            name: "x",
            ty: &F64_TYPE,
            ownership: Ownership::Owned,
        },
        ParamDescriptor {
            name: "y",
            ty: &F64_TYPE,
            ownership: Ownership::Owned,
        },
    ];
    static CONSTRUCTORS: [FunctionDescriptor<'static>; 1] = [FunctionDescriptor {
        name: "new",
        doc: Some("Creates a new point"),
        receiver: None,
        params: &NEW_PARAMS,
        return_type: &POINT_REF,
        return_ownership: Ownership::Owned,
        is_async: false,
        error_kind: None,
    }];
    static STRUCTS: [StructDescriptor<'static>; 1] = [StructDescriptor {
        id: TypeId::new("Point"),
        name: "Point",
        doc: None,
        fields: &POINT_FIELDS,
        methods: &[],
        constructors: &CONSTRUCTORS,
        properties: &[],
        trait_impls: &[],
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &[],
    }];

    let registry = TypeRegistry::new(&STRUCTS, &[], &[], &[]);
    let point = registry.get_struct(&TypeId::new("Point")).unwrap();
    assert_eq!(point.constructors.len(), 1);
    assert_eq!(point.constructors[0].name, "new");
    assert_eq!(point.constructors[0].params.len(), 2);
    assert!(point.constructors[0].receiver.is_none());
    assert!(registry.validate().is_ok());
}

#[test]
fn validate_catches_dangling_ref_in_constructor() {
    static MISSING: TypeDescriptor<'static> = TypeDescriptor::Ref(TypeId::new("Missing"));
    static CTOR_PARAMS: [ParamDescriptor<'static>; 1] = [ParamDescriptor {
        name: "val",
        ty: &MISSING,
        ownership: Ownership::Owned,
    }];
    static CTORS: [FunctionDescriptor<'static>; 1] = [FunctionDescriptor {
        name: "from_val",
        doc: None,
        receiver: None,
        params: &CTOR_PARAMS,
        return_type: &F64_TYPE,
        return_ownership: Ownership::Owned,
        is_async: false,
        error_kind: None,
    }];
    static STRUCTS: [StructDescriptor<'static>; 1] = [StructDescriptor {
        id: TypeId::new("Bad"),
        name: "Bad",
        doc: None,
        fields: &[],
        methods: &[],
        constructors: &CTORS,
        properties: &[],
        trait_impls: &[],
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &[],
    }];

    let registry = TypeRegistry::new(&STRUCTS, &[], &[], &[]);
    let errors = registry.validate().unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        &errors[0],
        RegistryError::DanglingRef { to, .. } if to.as_str() == "Missing"
    ));
}

#[test]
fn trait_impls_marker_traits() {
    static TRAITS: [TraitImpl<'static>; 4] = [
        TraitImpl::Display,
        TraitImpl::Debug,
        TraitImpl::Clone,
        TraitImpl::PartialEq,
    ];
    static STRUCTS: [StructDescriptor<'static>; 1] = [StructDescriptor {
        id: TypeId::new("Foo"),
        name: "Foo",
        doc: None,
        fields: &[],
        methods: &[],
        constructors: &[],
        properties: &[],
        trait_impls: &TRAITS,
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &[],
    }];

    let registry = TypeRegistry::new(&STRUCTS, &[], &[], &[]);
    let foo = registry.get_struct(&TypeId::new("Foo")).unwrap();
    assert_eq!(foo.trait_impls.len(), 4);
    assert_eq!(foo.trait_impls[0], TraitImpl::Display);
    assert!(registry.validate().is_ok());
}

#[test]
fn trait_impls_with_associated_types() {
    static ADD: TraitImpl<'static> = TraitImpl::Add {
        rhs: &F64_TYPE,
        output: &F64_TYPE,
    };
    static NEG: TraitImpl<'static> = TraitImpl::Neg { output: &F64_TYPE };
    static ITER_ITEM: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::I32);
    static ITER: TraitImpl<'static> = TraitImpl::Iterator { item: &ITER_ITEM };
    static TRAITS: [TraitImpl<'static>; 3] = [ADD, NEG, ITER];
    static STRUCTS: [StructDescriptor<'static>; 1] = [StructDescriptor {
        id: TypeId::new("Vec2"),
        name: "Vec2",
        doc: None,
        fields: &[],
        methods: &[],
        constructors: &[],
        properties: &[],
        trait_impls: &TRAITS,
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &[],
    }];

    let registry = TypeRegistry::new(&STRUCTS, &[], &[], &[]);
    assert!(registry.validate().is_ok());

    let vec2 = registry.get_struct(&TypeId::new("Vec2")).unwrap();
    assert!(matches!(
        &vec2.trait_impls[0],
        TraitImpl::Add { rhs, output }
        if **rhs == F64_TYPE && **output == F64_TYPE
    ));
}

#[test]
fn validate_catches_dangling_ref_in_trait_impl() {
    static MISSING: TypeDescriptor<'static> = TypeDescriptor::Ref(TypeId::new("Ghost"));
    static BAD_ADD: TraitImpl<'static> = TraitImpl::Add {
        rhs: &MISSING,
        output: &F64_TYPE,
    };
    static TRAITS: [TraitImpl<'static>; 1] = [BAD_ADD];
    static STRUCTS: [StructDescriptor<'static>; 1] = [StructDescriptor {
        id: TypeId::new("Bad"),
        name: "Bad",
        doc: None,
        fields: &[],
        methods: &[],
        constructors: &[],
        properties: &[],
        trait_impls: &TRAITS,
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &[],
    }];

    let registry = TypeRegistry::new(&STRUCTS, &[], &[], &[]);
    let errors = registry.validate().unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        &errors[0],
        RegistryError::DanglingRef { to, .. } if to.as_str() == "Ghost"
    ));
}

#[test]
fn property_descriptors() {
    static PROPS: [PropertyDescriptor<'static>; 2] = [
        PropertyDescriptor {
            name: "length",
            doc: Some("The magnitude of the vector"),
            ty: &F64_TYPE,
            readonly: true,
        },
        PropertyDescriptor {
            name: "angle",
            doc: None,
            ty: &F64_TYPE,
            readonly: false,
        },
    ];
    static STRUCTS: [StructDescriptor<'static>; 1] = [StructDescriptor {
        id: TypeId::new("Vec2"),
        name: "Vec2",
        doc: None,
        fields: &[],
        methods: &[],
        constructors: &[],
        properties: &PROPS,
        trait_impls: &[],
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &[],
    }];

    let registry = TypeRegistry::new(&STRUCTS, &[], &[], &[]);
    let v = registry.get_struct(&TypeId::new("Vec2")).unwrap();
    assert_eq!(v.properties.len(), 2);
    assert!(v.properties[0].readonly);
    assert!(!v.properties[1].readonly);
    assert!(registry.validate().is_ok());
}

#[test]
fn validate_catches_dangling_ref_in_property() {
    static MISSING: TypeDescriptor<'static> = TypeDescriptor::Ref(TypeId::new("Nope"));
    static PROPS: [PropertyDescriptor<'static>; 1] = [PropertyDescriptor {
        name: "bad",
        doc: None,
        ty: &MISSING,
        readonly: true,
    }];
    static STRUCTS: [StructDescriptor<'static>; 1] = [StructDescriptor {
        id: TypeId::new("Bad"),
        name: "Bad",
        doc: None,
        fields: &[],
        methods: &[],
        constructors: &[],
        properties: &PROPS,
        trait_impls: &[],
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &[],
    }];

    let registry = TypeRegistry::new(&STRUCTS, &[], &[], &[]);
    let errors = registry.validate().unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        &errors[0],
        RegistryError::DanglingRef { to, .. } if to.as_str() == "Nope"
    ));
}

#[test]
fn error_kind_on_function() {
    static ERR_TYPE: TypeDescriptor<'static> = TypeDescriptor::String;
    static RES_TYPE: TypeDescriptor<'static> = TypeDescriptor::Result(&I32_TYPE, &ERR_TYPE);
    static FN_DESC: FunctionDescriptor<'static> = FunctionDescriptor {
        name: "parse_int",
        doc: None,
        receiver: None,
        params: &[],
        return_type: &RES_TYPE,
        return_ownership: Ownership::Owned,
        is_async: false,
        error_kind: Some("ValueError"),
    };

    assert_eq!(FN_DESC.error_kind, Some("ValueError"));
    assert!(matches!(FN_DESC.return_type, TypeDescriptor::Result(..)));
}

#[test]
fn thread_safety_markers() {
    static STRUCTS: [StructDescriptor<'static>; 1] = [StructDescriptor {
        id: TypeId::new("RcWrapper"),
        name: "RcWrapper",
        doc: None,
        fields: &[],
        methods: &[],
        constructors: &[],
        properties: &[],
        trait_impls: &[],
        thread_safety: ThreadSafety::NONE,
        generic_params: &[],
    }];

    let registry = TypeRegistry::new(&STRUCTS, &[], &[], &[]);
    let w = registry.get_struct(&TypeId::new("RcWrapper")).unwrap();
    assert!(!w.thread_safety.is_send);
    assert!(!w.thread_safety.is_sync);

    // Verify associated constants
    const {
        assert!(ThreadSafety::SEND_SYNC.is_send && ThreadSafety::SEND_SYNC.is_sync);
        assert!(ThreadSafety::SEND.is_send && !ThreadSafety::SEND.is_sync);
        assert!(!ThreadSafety::NONE.is_send && !ThreadSafety::NONE.is_sync);
    }
}

#[test]
fn generic_type_params_on_struct() {
    static T_BOUNDS: [&str; 1] = ["Display"];
    static GENERIC_T: GenericParam<'static> = GenericParam {
        name: "T",
        bounds: &T_BOUNDS,
        default: None,
    };
    static GENERICS: [GenericParam<'static>; 1] = [GENERIC_T];
    static T_FIELD_TYPE: TypeDescriptor<'static> = TypeDescriptor::GenericParam("T");
    static FIELDS: [FieldDescriptor<'static>; 1] = [FieldDescriptor {
        name: "value",
        doc: None,
        ty: &T_FIELD_TYPE,
        readonly: false,
    }];
    static STRUCTS: [StructDescriptor<'static>; 1] = [StructDescriptor {
        id: TypeId::new("Container"),
        name: "Container",
        doc: None,
        fields: &FIELDS,
        methods: &[],
        constructors: &[],
        properties: &[],
        trait_impls: &[],
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &GENERICS,
    }];

    let registry = TypeRegistry::new(&STRUCTS, &[], &[], &[]);
    let c = registry.get_struct(&TypeId::new("Container")).unwrap();
    assert_eq!(c.generic_params.len(), 1);
    assert_eq!(c.generic_params[0].name, "T");
    assert_eq!(c.generic_params[0].bounds, &["Display"]);
    assert!(registry.validate().is_ok());
}

#[test]
fn generic_param_with_default() {
    static DEFAULT_TYPE: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::I32);
    static GENERIC: GenericParam<'static> = GenericParam {
        name: "T",
        bounds: &[],
        default: Some(&DEFAULT_TYPE),
    };
    static GENERICS: [GenericParam<'static>; 1] = [GENERIC];
    static T_TYPE: TypeDescriptor<'static> = TypeDescriptor::GenericParam("T");
    static FIELDS: [FieldDescriptor<'static>; 1] = [FieldDescriptor {
        name: "val",
        doc: None,
        ty: &T_TYPE,
        readonly: false,
    }];
    static STRUCTS: [StructDescriptor<'static>; 1] = [StructDescriptor {
        id: TypeId::new("WithDefault"),
        name: "WithDefault",
        doc: None,
        fields: &FIELDS,
        methods: &[],
        constructors: &[],
        properties: &[],
        trait_impls: &[],
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &GENERICS,
    }];

    let registry = TypeRegistry::new(&STRUCTS, &[], &[], &[]);
    let wd = registry.get_struct(&TypeId::new("WithDefault")).unwrap();
    assert!(wd.generic_params[0].default.is_some());
    assert!(registry.validate().is_ok());
}

#[test]
fn validate_catches_undeclared_generic_param() {
    // Field uses GenericParam("T") but struct declares no generic params
    static T_TYPE: TypeDescriptor<'static> = TypeDescriptor::GenericParam("T");
    static FIELDS: [FieldDescriptor<'static>; 1] = [FieldDescriptor {
        name: "val",
        doc: None,
        ty: &T_TYPE,
        readonly: false,
    }];
    static STRUCTS: [StructDescriptor<'static>; 1] = [StructDescriptor {
        id: TypeId::new("Bad"),
        name: "Bad",
        doc: None,
        fields: &FIELDS,
        methods: &[],
        constructors: &[],
        properties: &[],
        trait_impls: &[],
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &[], // No generics declared
    }];

    let registry = TypeRegistry::new(&STRUCTS, &[], &[], &[]);
    let errors = registry.validate().unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        &errors[0],
        RegistryError::UndeclaredGenericParam { owner, param_name }
        if owner.as_str() == "Bad" && *param_name == "T"
    ));
}

#[test]
fn validate_catches_dangling_ref_in_generic_default() {
    static MISSING: TypeDescriptor<'static> = TypeDescriptor::Ref(TypeId::new("Ghost"));
    static GENERIC: GenericParam<'static> = GenericParam {
        name: "T",
        bounds: &[],
        default: Some(&MISSING),
    };
    static GENERICS: [GenericParam<'static>; 1] = [GENERIC];
    static STRUCTS: [StructDescriptor<'static>; 1] = [StructDescriptor {
        id: TypeId::new("Bad"),
        name: "Bad",
        doc: None,
        fields: &[],
        methods: &[],
        constructors: &[],
        properties: &[],
        trait_impls: &[],
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &GENERICS,
    }];

    let registry = TypeRegistry::new(&STRUCTS, &[], &[], &[]);
    let errors = registry.validate().unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        &errors[0],
        RegistryError::DanglingRef { to, .. } if to.as_str() == "Ghost"
    ));
}

#[test]
fn type_alias_registration_and_lookup() {
    static INNER: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::F64);
    static ALIAS: TypeAliasDescriptor<'static> = TypeAliasDescriptor {
        id: TypeId::new("Meters"),
        name: "Meters",
        doc: Some("Distance in meters"),
        inner: &INNER,
        transparent: false,
    };

    let mut builder = TypeRegistryBuilder::new();
    builder.register_type_alias(ALIAS).unwrap();

    let registry = builder.as_registry();
    let meters = registry.get_type_alias(&TypeId::new("Meters")).unwrap();
    assert_eq!(meters.name, "Meters");
    assert!(!meters.transparent);

    // Unified lookup returns TypeAlias
    assert!(matches!(
        registry.get_type(&TypeId::new("Meters")),
        Some(TypeKind::TypeAlias(_))
    ));
}

#[test]
fn type_alias_transparent() {
    static INNER: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::F64);
    static ALIAS: TypeAliasDescriptor<'static> = TypeAliasDescriptor {
        id: TypeId::new("Seconds"),
        name: "Seconds",
        doc: None,
        inner: &INNER,
        transparent: true,
    };
    static ALIASES: [TypeAliasDescriptor<'static>; 1] = [ALIAS];

    let registry = TypeRegistry::new(&[], &[], &ALIASES, &[]);
    let s = registry.get_type_alias(&TypeId::new("Seconds")).unwrap();
    assert!(s.transparent);
    assert!(registry.validate().is_ok());
}

#[test]
fn type_alias_joins_known_types() {
    // A Ref to a type alias should not be dangling
    static INNER: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::F64);
    static ALIAS: TypeAliasDescriptor<'static> = TypeAliasDescriptor {
        id: TypeId::new("Meters"),
        name: "Meters",
        doc: None,
        inner: &INNER,
        transparent: false,
    };
    static ALIASES: [TypeAliasDescriptor<'static>; 1] = [ALIAS];

    static METERS_REF: TypeDescriptor<'static> = TypeDescriptor::Ref(TypeId::new("Meters"));
    static FIELDS: [FieldDescriptor<'static>; 1] = [FieldDescriptor {
        name: "distance",
        doc: None,
        ty: &METERS_REF,
        readonly: false,
    }];
    static STRUCTS: [StructDescriptor<'static>; 1] = [StructDescriptor {
        id: TypeId::new("Route"),
        name: "Route",
        doc: None,
        fields: &FIELDS,
        methods: &[],
        constructors: &[],
        properties: &[],
        trait_impls: &[],
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &[],
    }];

    let registry = TypeRegistry::new(&STRUCTS, &[], &ALIASES, &[]);
    assert!(registry.validate().is_ok());
}

#[test]
fn validate_catches_dangling_ref_in_alias_inner() {
    static MISSING: TypeDescriptor<'static> = TypeDescriptor::Ref(TypeId::new("Unknown"));
    static ALIAS: TypeAliasDescriptor<'static> = TypeAliasDescriptor {
        id: TypeId::new("BadAlias"),
        name: "BadAlias",
        doc: None,
        inner: &MISSING,
        transparent: false,
    };
    static ALIASES: [TypeAliasDescriptor<'static>; 1] = [ALIAS];

    let registry = TypeRegistry::new(&[], &[], &ALIASES, &[]);
    let errors = registry.validate().unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        &errors[0],
        RegistryError::DanglingRef { from, to }
        if from.as_str() == "BadAlias" && to.as_str() == "Unknown"
    ));
}

#[test]
fn duplicate_type_alias_errors() {
    static INNER: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::F64);
    static ALIAS: TypeAliasDescriptor<'static> = TypeAliasDescriptor {
        id: TypeId::new("Meters"),
        name: "Meters",
        doc: None,
        inner: &INNER,
        transparent: false,
    };

    let mut builder = TypeRegistryBuilder::new();
    builder.register_type_alias(ALIAS).unwrap();
    let result = builder.register_type_alias(ALIAS);
    assert!(matches!(result, Err(RegistryError::DuplicateType { .. })));
}

#[test]
fn alias_struct_cross_duplicate_errors() {
    static INNER: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::F64);
    static ALIAS: TypeAliasDescriptor<'static> = TypeAliasDescriptor {
        id: TypeId::new("Point"),
        name: "Point",
        doc: None,
        inner: &INNER,
        transparent: true,
    };

    let mut builder = TypeRegistryBuilder::new();
    Point::describe(&mut builder);
    let result = builder.register_type_alias(ALIAS);
    assert!(matches!(result, Err(RegistryError::DuplicateType { .. })));
}

#[test]
fn ownership_on_params_and_return() {
    static CLONE_PARAM: ParamDescriptor<'static> = ParamDescriptor {
        name: "data",
        ty: &STRING_TYPE,
        ownership: Ownership::Clone,
    };
    static REF_PARAM: ParamDescriptor<'static> = ParamDescriptor {
        name: "config",
        ty: &I32_TYPE,
        ownership: Ownership::Ref,
    };
    static PARAMS: [ParamDescriptor<'static>; 2] = [CLONE_PARAM, REF_PARAM];
    static FN_DESC: FunctionDescriptor<'static> = FunctionDescriptor {
        name: "process",
        doc: None,
        receiver: None,
        params: &PARAMS,
        return_type: &STRING_TYPE,
        return_ownership: Ownership::Owned,
        is_async: false,
        error_kind: None,
    };

    assert_eq!(FN_DESC.params[0].ownership, Ownership::Clone);
    assert_eq!(FN_DESC.params[1].ownership, Ownership::Ref);
    assert_eq!(FN_DESC.return_ownership, Ownership::Owned);
}

#[test]
fn generic_param_type_descriptor() {
    static GP: TypeDescriptor<'static> = TypeDescriptor::GenericParam("T");

    match &GP {
        TypeDescriptor::GenericParam(name) => assert_eq!(*name, "T"),
        other => panic!("expected GenericParam, got {other:?}"),
    }
}

#[test]
fn type_aliases_iterator() {
    static INNER: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::F64);
    static ALIAS: TypeAliasDescriptor<'static> = TypeAliasDescriptor {
        id: TypeId::new("Meters"),
        name: "Meters",
        doc: None,
        inner: &INNER,
        transparent: false,
    };

    let mut builder = TypeRegistryBuilder::new();
    builder.register_type_alias(ALIAS).unwrap();

    let registry = builder.as_registry();
    assert_eq!(registry.type_aliases().len(), 1);
}

#[test]
fn enum_with_all_new_fields() {
    static TRAITS: [TraitImpl<'static>; 2] = [TraitImpl::Display, TraitImpl::Hash];
    static BOUNDS: [&str; 1] = ["Clone"];
    static GENERICS: [GenericParam<'static>; 1] = [GenericParam {
        name: "T",
        bounds: &BOUNDS,
        default: None,
    }];
    static VARIANTS: [EnumVariant<'static>; 1] = [EnumVariant {
        name: "Some",
        doc: None,
        kind: VariantKind::Tuple(&[]),
    }];
    static ENUMS: [EnumDescriptor<'static>; 1] = [EnumDescriptor {
        id: TypeId::new("MyOption"),
        name: "MyOption",
        doc: None,
        variants: &VARIANTS,
        methods: &[],
        trait_impls: &TRAITS,
        thread_safety: ThreadSafety::SEND,
        generic_params: &GENERICS,
    }];

    let registry = TypeRegistry::new(&[], &ENUMS, &[], &[]);
    let opt = registry.get_enum(&TypeId::new("MyOption")).unwrap();
    assert_eq!(opt.trait_impls.len(), 2);
    assert_eq!(opt.thread_safety, ThreadSafety::SEND);
    assert_eq!(opt.generic_params.len(), 1);
    assert!(registry.validate().is_ok());
}

// ── Typestate and capability tests

#[test]
fn validate_returns_validated_registry() {
    let mut builder = TypeRegistryBuilder::new();
    Point::describe(&mut builder);
    Color::describe(&mut builder);

    let registry = builder.as_registry();
    let validated = registry.validate().unwrap();
    // Deref gives access to underlying registry methods
    assert!(validated.get_struct(&TypeId::new("Point")).is_some());
    assert!(validated.get_enum(&TypeId::new("Color")).is_some());
}

#[test]
fn capability_check_all_passes() {
    let mut builder = TypeRegistryBuilder::new();
    Point::describe(&mut builder);
    Color::describe(&mut builder);

    let registry = builder.as_registry();
    let validated = registry.validate().unwrap();
    assert!(BackendCapabilities::ALL.check(&validated).is_ok());
}

#[test]
fn capability_check_catches_unsupported_async() {
    static UNIT: TypeDescriptor<'static> = TypeDescriptor::Unit;
    static ASYNC_FN: [FunctionDescriptor<'static>; 1] = [FunctionDescriptor {
        name: "fetch",
        doc: None,
        receiver: Some(Receiver::Ref),
        params: &[],
        return_type: &UNIT,
        return_ownership: Ownership::Owned,
        is_async: true,
        error_kind: None,
    }];
    static STRUCTS: [StructDescriptor<'static>; 1] = [StructDescriptor {
        id: TypeId::new("Fetcher"),
        name: "Fetcher",
        doc: None,
        fields: &[],
        methods: &ASYNC_FN,
        constructors: &[],
        properties: &[],
        trait_impls: &[],
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &[],
    }];

    let registry = TypeRegistry::new(&STRUCTS, &[], &[], &[]);
    let validated = registry.validate().unwrap();

    let no_async = BackendCapabilities::ALL.with_async_fns(false);
    let errors = no_async.check(&validated).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        &errors[0],
        CompatibilityError::UnsupportedAsync { fn_name, .. } if *fn_name == "fetch"
    ));
}

#[test]
fn capability_check_catches_unsupported_callbacks() {
    static CB_PARAMS: [TypeDescriptor<'static>; 1] =
        [TypeDescriptor::Primitive(PrimitiveType::I32)];
    static UNIT_TD: TypeDescriptor<'static> = TypeDescriptor::Unit;
    static CB: TypeDescriptor<'static> = TypeDescriptor::Callback {
        params: &CB_PARAMS,
        return_type: &UNIT_TD,
    };
    static FIELDS: [FieldDescriptor<'static>; 1] = [FieldDescriptor {
        name: "handler",
        doc: None,
        ty: &CB,
        readonly: false,
    }];
    static STRUCTS: [StructDescriptor<'static>; 1] = [StructDescriptor {
        id: TypeId::new("Emitter"),
        name: "Emitter",
        doc: None,
        fields: &FIELDS,
        methods: &[],
        constructors: &[],
        properties: &[],
        trait_impls: &[],
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &[],
    }];

    let registry = TypeRegistry::new(&STRUCTS, &[], &[], &[]);
    let validated = registry.validate().unwrap();

    let no_cb = BackendCapabilities::ALL.with_callbacks(false);
    let errors = no_cb.check(&validated).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        &errors[0],
        CompatibilityError::UnsupportedCallback { context, .. } if *context == "handler"
    ));
}

#[test]
fn capability_check_catches_unsupported_generics() {
    static BOUNDS: [&str; 0] = [];
    static GENERICS: [GenericParam<'static>; 1] = [GenericParam {
        name: "T",
        bounds: &BOUNDS,
        default: None,
    }];
    static T_TYPE: TypeDescriptor<'static> = TypeDescriptor::GenericParam("T");
    static FIELDS: [FieldDescriptor<'static>; 1] = [FieldDescriptor {
        name: "val",
        doc: None,
        ty: &T_TYPE,
        readonly: false,
    }];
    static STRUCTS: [StructDescriptor<'static>; 1] = [StructDescriptor {
        id: TypeId::new("Generic"),
        name: "Generic",
        doc: None,
        fields: &FIELDS,
        methods: &[],
        constructors: &[],
        properties: &[],
        trait_impls: &[],
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &GENERICS,
    }];

    let registry = TypeRegistry::new(&STRUCTS, &[], &[], &[]);
    let validated = registry.validate().unwrap();

    let no_generics = BackendCapabilities::ALL.with_generics(false);
    let errors = no_generics.check(&validated).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        &errors[0],
        CompatibilityError::UnsupportedGenerics { type_id } if type_id.as_str() == "Generic"
    ));
}

#[test]
fn capability_check_catches_unsupported_properties() {
    static PROPS: [PropertyDescriptor<'static>; 1] = [PropertyDescriptor {
        name: "length",
        doc: None,
        ty: &F64_TYPE,
        readonly: true,
    }];
    static STRUCTS: [StructDescriptor<'static>; 1] = [StructDescriptor {
        id: TypeId::new("Vec2"),
        name: "Vec2",
        doc: None,
        fields: &[],
        methods: &[],
        constructors: &[],
        properties: &PROPS,
        trait_impls: &[],
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &[],
    }];

    let registry = TypeRegistry::new(&STRUCTS, &[], &[], &[]);
    let validated = registry.validate().unwrap();

    let no_props = BackendCapabilities::ALL.with_properties(false);
    let errors = no_props.check(&validated).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        &errors[0],
        CompatibilityError::UnsupportedProperties { .. }
    ));
}

#[test]
fn capability_check_catches_unsupported_type_alias() {
    static INNER: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::F64);
    static ALIASES: [TypeAliasDescriptor<'static>; 1] = [TypeAliasDescriptor {
        id: TypeId::new("Meters"),
        name: "Meters",
        doc: None,
        inner: &INNER,
        transparent: false,
    }];

    let registry = TypeRegistry::new(&[], &[], &ALIASES, &[]);
    let validated = registry.validate().unwrap();

    let no_aliases = BackendCapabilities::ALL.with_type_aliases(false);
    let errors = no_aliases.check(&validated).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        &errors[0],
        CompatibilityError::UnsupportedTypeAlias { type_id } if type_id.as_str() == "Meters"
    ));
}

#[test]
fn capability_check_catches_insufficient_thread_safety() {
    static STRUCTS: [StructDescriptor<'static>; 1] = [StructDescriptor {
        id: TypeId::new("NotSync"),
        name: "NotSync",
        doc: None,
        fields: &[],
        methods: &[],
        constructors: &[],
        properties: &[],
        trait_impls: &[],
        thread_safety: ThreadSafety::SEND, // Send but not Sync
        generic_params: &[],
    }];

    let registry = TypeRegistry::new(&STRUCTS, &[], &[], &[]);
    let validated = registry.validate().unwrap();

    let needs_send_sync = BackendCapabilities::ALL
        .with_required_thread_safety(Some(ThreadSafety::SEND_SYNC));
    let errors = needs_send_sync.check(&validated).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        &errors[0],
        CompatibilityError::InsufficientThreadSafety { type_id, .. }
        if type_id.as_str() == "NotSync"
    ));
}

#[test]
fn capability_check_thread_safety_passes_when_met() {
    static STRUCTS: [StructDescriptor<'static>; 1] = [StructDescriptor {
        id: TypeId::new("Safe"),
        name: "Safe",
        doc: None,
        fields: &[],
        methods: &[],
        constructors: &[],
        properties: &[],
        trait_impls: &[],
        thread_safety: ThreadSafety::SEND_SYNC,
        generic_params: &[],
    }];

    let registry = TypeRegistry::new(&STRUCTS, &[], &[], &[]);
    let validated = registry.validate().unwrap();

    let needs_send = BackendCapabilities::ALL
        .with_required_thread_safety(Some(ThreadSafety::SEND));
    assert!(needs_send.check(&validated).is_ok());
}
