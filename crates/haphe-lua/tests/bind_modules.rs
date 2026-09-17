//! Integration test: registry → validate → LuaBinder → Lua runtime.

use haphe::{
    ConstantDescriptor, FieldDescriptor, FunctionDescriptor, ModuleDescriptor, Ownership,
    ParamDescriptor, PrimitiveType, RuntimeBinder, StructDescriptor, ThreadSafety, TypeDescriptor,
    TypeId, TypeRegistry,
};
use haphe_lua::LuaBinder;
use mlua::Lua;

// type descriptors

const I32: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::I32);
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

static ADD_FN: FunctionDescriptor<'static> = FunctionDescriptor {
    name: "add",
    doc: Some("Adds two integers."),
    receiver: None,
    generic_params: &[],
    instantiations: &[],
    dispatch: haphe::Dispatch::Static,
    params: &[
        ParamDescriptor {
            name: "a",
            ty: &I32,
            ownership: Ownership::Owned,
        },
        ParamDescriptor {
            name: "b",
            ty: &I32,
            ownership: Ownership::Owned,
        },
    ],
    return_type: &I32,
    return_ownership: Ownership::Owned,
    is_async: false,
    error_kind: None,
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
    ty: &I32,
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
    value: "math",
};

static INNER_MODULE: ModuleDescriptor<'static> = ModuleDescriptor {
    name: "trig",
    doc: None,
    functions: &[],
    type_ids: &[],
    submodules: &[],
    constants: &[],
    function_instantiations: &[],
};

static MATH_MODULE: ModuleDescriptor<'static> = ModuleDescriptor {
    name: "math",
    doc: Some("Math functions"),
    functions: &[ADD_FN],
    type_ids: &[TypeId::new("test::Point")],
    submodules: &[INNER_MODULE],
    constants: &[PI_CONST, MAX_CONST, ENABLED_CONST, NAME_CONST],
    function_instantiations: &[],
};

static REGISTRY: TypeRegistry<'static> =
    TypeRegistry::new(&[POINT], &[], &[], &[MATH_MODULE], &[], &[]);

#[test]
fn module_table_is_registered_as_global() {
    let mut lua = Lua::new();
    let binder = LuaBinder::new();
    let validated = REGISTRY.validate().unwrap();
    binder.bind(&validated, &mut lua).unwrap();

    let math: mlua::Table = lua.globals().get("math").unwrap();
    assert!(math.get::<mlua::Value>("PI").is_ok());
}

#[test]
fn constants_have_correct_values() {
    let mut lua = Lua::new();
    let binder = LuaBinder::new();
    let validated = REGISTRY.validate().unwrap();
    binder.bind(&validated, &mut lua).unwrap();

    let math: mlua::Table = lua.globals().get("math").unwrap();

    let pi: f64 = math.get("PI").unwrap();
    assert!((pi - std::f64::consts::PI).abs() < 1e-10);

    let max: i64 = math.get("MAX").unwrap();
    assert_eq!(max, 100);

    let enabled: bool = math.get("ENABLED").unwrap();
    assert!(enabled);

    let name: String = math.get("NAME").unwrap();
    assert_eq!(name, "math");
}

#[test]
fn type_stub_is_present() {
    let mut lua = Lua::new();
    let binder = LuaBinder::new();
    let validated = REGISTRY.validate().unwrap();
    binder.bind(&validated, &mut lua).unwrap();

    let math: mlua::Table = lua.globals().get("math").unwrap();
    let point: mlua::Table = math.get("Point").unwrap();
    // The stub is an empty table for now.
    assert_eq!(point.len().unwrap(), 0);
}

#[test]
fn submodule_is_nested() {
    let mut lua = Lua::new();
    let binder = LuaBinder::new();
    let validated = REGISTRY.validate().unwrap();
    binder.bind(&validated, &mut lua).unwrap();

    let math: mlua::Table = lua.globals().get("math").unwrap();
    let _trig: mlua::Table = math.get("trig").unwrap();
}

#[test]
fn function_stubs_error_with_message() {
    let mut lua = Lua::new();
    let binder = LuaBinder::new();
    let validated = REGISTRY.validate().unwrap();
    binder.bind(&validated, &mut lua).unwrap();

    let result: mlua::Result<()> = lua.load("math.add(1, 2)").exec();
    let err = result.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("not yet implemented"),
        "unexpected error: {msg}"
    );
}

#[test]
fn lua_script_can_read_constants() {
    let mut lua = Lua::new();
    let binder = LuaBinder::new();
    let validated = REGISTRY.validate().unwrap();
    binder.bind(&validated, &mut lua).unwrap();

    let result: f64 = lua.load("return math.PI * 2").eval().unwrap();
    assert!((result - std::f64::consts::PI * 2.0).abs() < 1e-10);
}
