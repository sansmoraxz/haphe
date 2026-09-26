//! Integration test: registry, validate, `RhaiBinder`, Rhai runtime —
//! modules, constants, and submodules.

use haphe::{
    ConstantDescriptor, FieldDescriptor, ModuleDescriptor, PrimitiveType, RuntimeBinder,
    StructDescriptor, ThreadSafety, TypeDescriptor, TypeId, TypeRegistry,
};
use haphe_rhai::RhaiBinder;

const I64: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::I64);
const F64: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::F64);
const BOOL: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::Bool);
const STR: TypeDescriptor<'static> = TypeDescriptor::String;

static POINT: StructDescriptor<'static> = StructDescriptor {
    id: TypeId::new("test::Point"),
    name: "Point",
    doc: Some("A 2D point"),
    fields: &[
        FieldDescriptor {
            name: "x",
            doc: None,
            ty: &F64,
            readonly: false,
        },
        FieldDescriptor {
            name: "y",
            doc: None,
            ty: &F64,
            readonly: true,
        },
    ],
    methods: &[],
    constructors: &[],
    properties: &[],
    trait_impls: &[],
    thread_safety: ThreadSafety::SEND_SYNC,
    generic_params: &[],
};

static PI_CONST: ConstantDescriptor<'static> = ConstantDescriptor {
    name: "PI",
    doc: Some("Pi"),
    ty: &F64,
    value: "3.141592653589793",
};

static MAX_CONST: ConstantDescriptor<'static> = ConstantDescriptor {
    name: "MAX",
    doc: None,
    ty: &I64,
    value: "100",
};

static ENABLED_CONST: ConstantDescriptor<'static> = ConstantDescriptor {
    name: "ENABLED",
    doc: None,
    ty: &BOOL,
    value: "true",
};

static NAME_CONST: ConstantDescriptor<'static> = ConstantDescriptor {
    name: "NAME",
    doc: None,
    ty: &STR,
    value: "geo",
};

static TRIG_TAU_CONST: ConstantDescriptor<'static> = ConstantDescriptor {
    name: "TAU",
    doc: None,
    ty: &F64,
    value: "6.283185307179586",
};

static TRIG_MODULE: ModuleDescriptor<'static> = ModuleDescriptor {
    name: "trig",
    doc: None,
    functions: &[],
    type_ids: &[],
    submodules: &[],
    constants: &[TRIG_TAU_CONST],
    function_instantiations: &[],
};

static GEOMETRY_MODULE: ModuleDescriptor<'static> = ModuleDescriptor {
    name: "geometry",
    doc: Some("Geometry module"),
    functions: &[],
    type_ids: &[TypeId::new("test::Point")],
    submodules: &[TRIG_MODULE],
    constants: &[PI_CONST, MAX_CONST, ENABLED_CONST, NAME_CONST],
    function_instantiations: &[],
};

static REGISTRY: TypeRegistry<'static> =
    TypeRegistry::new(&[POINT], &[], &[], &[GEOMETRY_MODULE], &[], &[]);

fn bound_engine() -> rhai::Engine {
    let mut engine = rhai::Engine::new();
    let binder = RhaiBinder::new();
    let validated = REGISTRY.validate().expect("validates");
    binder.bind(&validated, &mut engine).expect("binds");
    engine
}

#[test]
fn module_is_registered_as_static_module() {
    let engine = bound_engine();
    let pi: f64 = engine.eval("geometry::PI").expect("eval");
    assert!((pi - std::f64::consts::PI).abs() < 1e-10);
}

#[test]
fn constants_have_correct_values() {
    let engine = bound_engine();

    let pi: f64 = engine.eval("geometry::PI").expect("eval");
    assert!((pi - std::f64::consts::PI).abs() < 1e-10);

    let max: i64 = engine.eval("geometry::MAX").expect("eval");
    assert_eq!(max, 100);

    let enabled: bool = engine.eval("geometry::ENABLED").expect("eval");
    assert!(enabled);

    let name: String = engine.eval("geometry::NAME").expect("eval");
    assert_eq!(name, "geo");
}

#[test]
fn submodule_is_nested() {
    let engine = bound_engine();
    let tau: f64 = engine
        .eval("geometry::trig::TAU")
        .expect("eval");
    assert!((tau - std::f64::consts::TAU).abs() < 1e-10);
}

#[test]
fn language_name() {
    let binder = RhaiBinder::new();
    assert_eq!(binder.language_name(), "rhai");
}
