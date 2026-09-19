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

/// Host-side callbacks.
#[script(foreign)]
pub trait Notifier {
    fn notify(&self, message: String);
}

haphe::registry! {
    pub static REGISTRY = {
        structs: [Point],
        enums: [Color],
        foreign: [NotifierHandle],
        modules: [
            mod geometry {
                doc: "Geometry utilities",
                functions: [
                    add,
                    math::mul,
                    /// Doubles a value.
                    double: |x: i64| -> i64 { x * 2 },
                    halve_later: async |x: i64| -> i64 { x / 2 },
                    #[script(error_kind = "SignError")]
                    checked_neg: |x: i64| -> Result<i64, NegError> {
                        x.checked_neg().ok_or(NegError)
                    },
                ],
                types: [Point, Color],
                constants: [
                    /// Default scale factor.
                    SCALE: f64 = 2.5,
                    /// Spelled with separators and a suffix; the descriptor
                    /// carries the canonical value backends can `parse`.
                    LIMIT: f64 = 1_000.5f64,
                    MAX_ITEMS: i64 = 25_000i64,
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
    assert_eq!(module.functions.len(), 5);
    assert_eq!(module.functions[0].name, "add");
    assert_eq!(module.functions[1].name, "mul");
    assert_eq!(module.functions[2].name, "double");
    assert_eq!(module.type_ids.len(), 2);
    assert_eq!(module.submodules.len(), 1);
    assert_eq!(module.submodules[0].functions[0].name, "add");
    let scale = &module.constants[0];
    assert_eq!(scale.name, "SCALE");
    assert_eq!(scale.doc, Some("Default scale factor."));
    assert_eq!(*scale.ty, TypeDescriptor::Primitive(PrimitiveType::F64));
    assert_eq!(scale.value, "2.5");
    let limit = &module.constants[1];
    assert_eq!(limit.value, "1000.5");
    assert_eq!(limit.value.parse::<f64>().ok(), Some(1000.5));
    let max_items = &module.constants[2];
    assert_eq!(max_items.value, "25000");
    assert_eq!(max_items.value.parse::<i64>().ok(), Some(25000));

    let notifier = &REGISTRY.foreign_interfaces()[0];
    assert_eq!(notifier.name, "Notifier");
    assert_eq!(notifier.doc, Some("Host-side callbacks."));
    assert_eq!(notifier.functions[0].name, "notify");
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
        2,
        "only Color and Notifier (both `none`) should fail: {errors:?}"
    );
}

#[derive(Debug)]
struct NegError;

impl std::fmt::Display for NegError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("i64::MIN has no negation")
    }
}

impl std::error::Error for NegError {}

/// Registry closures expand into ordinary free fns inside generated Rust
/// modules mirroring the registry tree (`geometry::double`): descriptors
/// carry the entry's docs and options, and the generated carriers bind
/// through the standard `ScriptBindFn` machinery.
#[test]
fn registry_closures_describe_and_bind() {
    use haphe::{FnBinder, ScriptBindFn, ScriptCallError, ScriptValue};

    #[derive(Debug)]
    struct NeverError;
    impl std::fmt::Display for NeverError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("never")
        }
    }
    impl std::error::Error for NeverError {}

    type SyncFn = fn(&[ScriptValue]) -> Result<ScriptValue, ScriptCallError>;
    type AsyncFreeFn = for<'a> fn(&'a [ScriptValue]) -> haphe::ScriptCallFuture<'a>;
    #[derive(Default)]
    struct Collect {
        sync: Vec<(&'static str, SyncFn)>,
        asynchronous: Vec<&'static str>,
    }
    impl FnBinder for Collect {
        type Error = NeverError;
        fn function(
            &mut self,
            name: &'static str,
            _: &'static [haphe::TypeDescriptor<'static>],
            f: SyncFn,
        ) -> Result<(), NeverError> {
            self.sync.push((name, f));
            Ok(())
        }

        fn function_async(
            &mut self,
            name: &'static str,
            _: &'static [haphe::TypeDescriptor<'static>],
            _: AsyncFreeFn,
        ) -> Result<(), NeverError> {
            self.asynchronous.push(name);
            Ok(())
        }
    }

    let module = &REGISTRY.modules()[0];
    let double_desc = module
        .functions
        .iter()
        .find(|f| f.name == "double")
        .expect("closure described");
    assert_eq!(double_desc.doc, Some("Doubles a value."));
    assert!(!double_desc.fallible);
    let halve_desc = module
        .functions
        .iter()
        .find(|f| f.name == "halve_later")
        .expect("async closure described");
    assert!(halve_desc.is_async);
    let neg_desc = module
        .functions
        .iter()
        .find(|f| f.name == "checked_neg")
        .expect("fallible closure described");
    assert!(neg_desc.fallible);
    assert_eq!(neg_desc.error_kind, Some("SignError"));

    let mut binder = Collect::default();
    <geometry::double as ScriptBindFn>::bind(&mut binder).unwrap();
    <geometry::halve_later as ScriptBindFn>::bind(&mut binder).unwrap();
    <geometry::checked_neg as ScriptBindFn>::bind(&mut binder).unwrap();

    let (_, f) = binder.sync.iter().find(|(n, _)| *n == "double").unwrap();
    let out = f(&[ScriptValue::I64(21)]).unwrap();
    assert!(matches!(out, ScriptValue::I64(42)));
    assert!(binder.asynchronous.contains(&"halve_later"));

    let (_, checked) = binder
        .sync
        .iter()
        .find(|(n, _)| *n == "checked_neg")
        .unwrap();
    let out = checked(&[ScriptValue::I64(5)]).unwrap();
    assert!(matches!(out, ScriptValue::I64(-5)));
    match checked(&[ScriptValue::I64(i64::MIN)]).unwrap_err() {
        ScriptCallError::Callee { error, kind, .. } => {
            assert_eq!(kind, Some("SignError"));
            assert!(error.downcast_ref::<NegError>().is_some());
        }
        other @ ScriptCallError::Convert(_) => panic!("expected Callee error, got {other:?}"),
    }
}
