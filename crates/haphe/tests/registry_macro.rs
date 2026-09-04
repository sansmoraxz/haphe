//! End-to-end: derive + impl blocks + free fns assembled by `registry!`,
//! validated, and checked against backend capabilities.

#![cfg(feature = "macros")]

use haphe::{
    BackendCapabilities, PrimitiveType, Script, ScriptType, ThreadSafety, TypeDescriptor, TypeId,
    script,
};

/// A 2D point.
#[derive(Script)]
#[script(thread_safety = send_sync, methods)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[script]
impl Point {
    #[script(constructor)]
    pub fn new(x: f64, y: f64) -> Self {
        Point { x, y }
    }

    pub fn nearest(&self, candidates: Vec<Point>) -> Option<Point> {
        candidates.into_iter().next()
    }
}

/// A named color.
#[derive(Script)]
pub enum Color {
    Red,
    Rgb(u8, u8, u8),
    Named { name: String },
}

/// Adds two numbers.
#[script]
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

pub mod math {
    use haphe::script;

    #[script]
    pub fn mul(a: f64, b: f64) -> f64 {
        a * b
    }
}

haphe::registry! {
    pub static REGISTRY = {
        structs: [Point],
        enums: [Color],
        modules: [
            mod geometry {
                doc: "Geometry utilities",
                functions: [add, math::mul],
                types: [Point, Color],
                constants: [
                    /// Default scale factor.
                    SCALE: f64 = 2.5,
                ],
                modules: [
                    mod inner {
                        functions: [add],
                    },
                ],
            },
        ],
    };
}

#[test]
fn registry_validates() {
    let validated = REGISTRY.validate().expect("registry is structurally valid");
    BackendCapabilities::ALL
        .check(&validated)
        .expect("compatible with a full-featured backend");
}

#[test]
fn registry_contents() {
    assert_eq!(REGISTRY.structs().len(), 1);
    assert_eq!(REGISTRY.enums().len(), 1);
    let point = REGISTRY.get_struct(&<Point as ScriptType>::ID).unwrap();
    assert_eq!(point.name, "Point");
    assert_eq!(point.constructors.len(), 1);
    assert_eq!(point.methods.len(), 1);
    // The method's parameter and return types reference the registered type.
    let nearest = &point.methods[0];
    assert_eq!(
        *nearest.return_type,
        TypeDescriptor::Option(&TypeDescriptor::Ref(TypeId::new("registry_macro::Point"))),
    );

    let color = REGISTRY.get_enum(&<Color as ScriptType>::ID).unwrap();
    assert_eq!(color.variants.len(), 3);
    assert_eq!(color.thread_safety, ThreadSafety::NONE);

    let module = &REGISTRY.modules()[0];
    assert_eq!(module.name, "geometry");
    assert_eq!(module.doc, Some("Geometry utilities"));
    assert_eq!(module.functions.len(), 2);
    assert_eq!(module.functions[0].name, "add");
    assert_eq!(module.functions[1].name, "mul");
    assert_eq!(module.type_ids.len(), 2);
    assert_eq!(module.submodules.len(), 1);
    assert_eq!(module.submodules[0].functions[0].name, "add");
    let scale = &module.constants[0];
    assert_eq!(scale.name, "SCALE");
    assert_eq!(scale.doc, Some("Default scale factor."));
    assert_eq!(*scale.ty, TypeDescriptor::Primitive(PrimitiveType::F64));
    assert_eq!(scale.value, "2.5");
}

/// A backend that can't do properties still accepts this registry (none used
/// beyond capabilities), while one requiring Send+Sync rejects `Color`.
#[test]
fn capability_check_catches_thread_safety() {
    let validated = REGISTRY.validate().unwrap();
    let strict =
        BackendCapabilities::ALL.with_required_thread_safety(Some(ThreadSafety::SEND_SYNC));
    let errors = strict.check(&validated).unwrap_err();
    assert_eq!(
        errors.len(),
        1,
        "only Color (default `none`) should fail: {errors:?}"
    );
}
