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
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[]);

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
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[]);

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
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[]);

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
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[]);

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
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[]);

    match haphe::generate(&WitGenerator::new("haphe:demo"), &REGISTRY) {
        Err(GenerateError::Incompatible(_)) => {}
        other => panic!("expected Incompatible, got: {other:?}"),
    }
}

#[test]
fn async_fn_rejected_by_capabilities() {
    static METHODS: [FunctionDescriptor; 1] = [FunctionDescriptor {
        name: "fetch",
        doc: None,
        receiver: Some(Receiver::Ref),
        params: &[],
        return_type: &F64,
        return_ownership: Ownership::Owned,
        is_async: true,
        error_kind: None,
    }];
    static STRUCTS: [StructDescriptor; 1] = [StructDescriptor {
        methods: &METHODS,
        ..plain_struct("test::Client", "Client")
    }];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[]);

    match haphe::generate(&WitGenerator::new("haphe:demo"), &REGISTRY) {
        Err(GenerateError::Incompatible(_)) => {}
        other => panic!("expected Incompatible, got: {other:?}"),
    }
}
