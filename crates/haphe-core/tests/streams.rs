//! Capability checks for first-class stream/future descriptor types.

use haphe_core::{
    BackendCapabilities, CompatibilityError, FieldDescriptor, PrimitiveType, StructDescriptor,
    ThreadSafety, TypeDescriptor, TypeId, TypeRegistry,
};

const I32: TypeDescriptor = TypeDescriptor::Primitive(PrimitiveType::I32);

static STREAM_I32: TypeDescriptor = TypeDescriptor::Stream(&I32);
static FIELDS: [FieldDescriptor; 1] = [FieldDescriptor {
    name: "events",
    doc: None,
    ty: &STREAM_I32,
    readonly: false,
}];
static STRUCTS: [StructDescriptor; 1] = [StructDescriptor {
    id: TypeId::new("test::Feed"),
    name: "Feed",
    doc: None,
    fields: &FIELDS,
    methods: &[],
    constructors: &[],
    properties: &[],
    trait_impls: &[],
    thread_safety: ThreadSafety::SEND_SYNC,
    generic_params: &[],
}];
static REGISTRY: TypeRegistry = TypeRegistry::new(&STRUCTS, &[], &[], &[], &[], &[]);

#[test]
fn streams_capability_accepts_and_rejects() {
    let validated = REGISTRY.validate().unwrap();
    BackendCapabilities::ALL
        .check(&validated)
        .expect("streams supported by default");

    let errors = BackendCapabilities::ALL
        .with_streams(false)
        .check(&validated)
        .expect_err("stream field must be rejected");
    assert!(
        errors.iter().any(|e| matches!(
            e,
            CompatibilityError::UnsupportedStream {
                context: "events",
                ..
            }
        )),
        "got: {errors:?}"
    );
}

#[test]
fn module_stream_fn_rejected_without_capability() {
    use haphe_core::{FunctionDescriptor, ModuleDescriptor, Ownership, ParamDescriptor};
    static FNS: [FunctionDescriptor; 1] = [FunctionDescriptor {
        name: "subscribe",
        doc: None,
        receiver: None,
        generic_params: &[],
        instantiations: &[],
        params: &[ParamDescriptor {
            name: "topic",
            ty: &TypeDescriptor::String,
            ownership: Ownership::Owned,
        }],
        return_type: &STREAM_I32,
        return_ownership: Ownership::Owned,
        is_async: false,
        error_kind: None,
    }];
    static MODULES: [ModuleDescriptor; 1] = [ModuleDescriptor {
        name: "events",
        doc: None,
        functions: &FNS,
        type_ids: &[],
        submodules: &[],
        constants: &[],
    }];
    static REGISTRY: TypeRegistry = TypeRegistry::new(&[], &[], &[], &MODULES, &[], &[]);

    let validated = REGISTRY.validate().unwrap();
    let errors = BackendCapabilities::ALL
        .with_streams(false)
        .check(&validated)
        .expect_err("stream-returning module fn must be rejected");
    assert!(
        errors.iter().any(|e| matches!(
            e,
            CompatibilityError::UnsupportedModuleStream {
                context: "subscribe",
                ..
            }
        )),
        "got: {errors:?}"
    );
}

static FUTURE_I32: TypeDescriptor = TypeDescriptor::Future(&I32);
static FUT_FIELDS: [FieldDescriptor; 1] = [FieldDescriptor {
    name: "pending",
    doc: None,
    ty: &FUTURE_I32,
    readonly: false,
}];
static FUT_STRUCTS: [StructDescriptor; 1] = [StructDescriptor {
    id: TypeId::new("test::Deferred"),
    name: "Deferred",
    doc: None,
    fields: &FUT_FIELDS,
    methods: &[],
    constructors: &[],
    properties: &[],
    trait_impls: &[],
    thread_safety: ThreadSafety::SEND_SYNC,
    generic_params: &[],
}];
static FUT_REGISTRY: TypeRegistry = TypeRegistry::new(&FUT_STRUCTS, &[], &[], &[], &[], &[]);

/// Streams and futures are independent capabilities: disabling one leaves
/// the other accepted.
#[test]
fn stream_and_future_capabilities_are_independent() {
    let streams_reg = REGISTRY.validate().unwrap();
    let futures_reg = FUT_REGISTRY.validate().unwrap();

    // Futures-only backend: stream field rejected, future field fine.
    let caps = BackendCapabilities::ALL.with_streams(false);
    caps.check(&futures_reg).expect("future field accepted");
    assert!(caps.check(&streams_reg).is_err(), "stream field rejected");

    // Streams-only backend: the reverse.
    let caps = BackendCapabilities::ALL.with_futures(false);
    caps.check(&streams_reg).expect("stream field accepted");
    let errors = caps.check(&futures_reg).expect_err("future field rejected");
    assert!(
        errors.iter().any(|e| matches!(
            e,
            CompatibilityError::UnsupportedFuture {
                context: "pending",
                ..
            }
        )),
        "got: {errors:?}"
    );
}
