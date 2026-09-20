//! Generic `#[script(foreign)]` traits: erased descriptors with
//! `GenericParam` placeholders, instantiation records via `registry!`, and
//! per-instantiation runtime dispatch.

#![cfg(feature = "macros")]

use haphe::{
    BackendCapabilities, CompatibilityError, ForeignCaller, ForeignError, ForeignHandle,
    PrimitiveType, ScriptForeign, ScriptValue, TypeDescriptor, script,
};

/// A host-backed key-value store.
#[script(foreign, thread_safety = none)]
pub trait Store<T> {
    fn get(&self, key: String) -> Option<T>;
    fn put(&self, key: String, value: T);
    async fn refresh(&self) -> Result<T, StoreError>;
}

#[derive(Debug)]
pub struct StoreError(String);

impl From<ForeignError> for StoreError {
    fn from(e: ForeignError) -> Self {
        Self(e.to_string())
    }
}

/// Bounds on the trait are recorded in the descriptor.
#[script(foreign)]
pub trait Cache<T: Clone> {
    fn lookup(&self, key: String) -> Option<T>;
}

haphe::registry! {
    pub static REGISTRY = {
        foreign: [StoreHandle<i32>, StoreHandle<String>],
    };
}

#[test]
fn descriptor_is_erased_and_uniform_across_instantiations() {
    let desc = <StoreHandle<i32> as ScriptForeign>::DESCRIPTOR;
    assert_eq!(desc, <StoreHandle<String> as ScriptForeign>::DESCRIPTOR);
    assert_eq!(desc.generic_params.len(), 1);
    assert_eq!(desc.generic_params[0].name, "T");
    assert!(desc.generic_params[0].bounds.is_empty());

    let get = &desc.functions[0];
    assert_eq!(
        *get.return_type,
        TypeDescriptor::Option(&TypeDescriptor::GenericParam("T"))
    );
    let put = &desc.functions[1];
    assert_eq!(*put.params[1].ty, TypeDescriptor::GenericParam("T"));
    // Result<T, E> is described as its Ok type.
    let refresh = &desc.functions[2];
    assert!(refresh.is_async);
    assert_eq!(*refresh.return_type, TypeDescriptor::GenericParam("T"));
}

#[test]
fn bounds_are_recorded() {
    let desc = <CacheHandle<String> as ScriptForeign>::DESCRIPTOR;
    assert_eq!(desc.generic_params[0].bounds, &["Clone"]);
}

#[test]
fn registry_records_one_interface_and_two_instantiations() {
    assert_eq!(REGISTRY.foreign_interfaces().len(), 1);
    let desc = <StoreHandle<i32> as ScriptForeign>::DESCRIPTOR;
    let insts = REGISTRY.instantiations();
    assert_eq!(insts.len(), 2);
    assert!(insts.iter().all(|i| i.id == desc.id));
    assert_eq!(
        *insts[0].args,
        [TypeDescriptor::Primitive(PrimitiveType::I32)]
    );
    assert_eq!(*insts[1].args, [TypeDescriptor::String]);

    let validated = REGISTRY.validate().expect("registry validates");
    BackendCapabilities::ALL
        .check(&validated)
        .expect("compatible with a full-featured backend");
}

#[test]
fn generics_capability_gates_foreign_interfaces() {
    let validated = REGISTRY.validate().unwrap();
    let errors = BackendCapabilities::ALL
        .with_generics(false)
        .check(&validated)
        .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, CompatibilityError::UnsupportedGenerics { .. }))
    );
}

// ── Runtime dispatch: the caller is built per instantiation; dispatch uses
// plain method names.

type Respond =
    fn(&str, &[TypeDescriptor<'static>], &[ScriptValue]) -> Result<ScriptValue, ForeignError>;

struct MapCaller(Respond);

impl ForeignCaller for MapCaller {
    fn call(
        &self,
        function: &'static str,
        type_args: &[TypeDescriptor<'static>],
        args: &[ScriptValue],
    ) -> Result<ScriptValue, ForeignError> {
        (self.0)(function, type_args, args)
    }

    fn call_async<'a>(
        &'a self,
        function: &'static str,
        type_args: &'a [TypeDescriptor<'static>],
        args: &'a [ScriptValue],
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<ScriptValue, ForeignError>> + 'a>> {
        Box::pin(std::future::ready(self.call(function, type_args, args)))
    }
}

fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = std::pin::pin!(fut);
    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);
    loop {
        if let std::task::Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
            return out;
        }
    }
}

#[test]
fn dispatch_at_two_instantiations() {
    let ints: StoreHandle<i32> =
        StoreHandle::from_caller(Box::new(MapCaller(|f, type_args, args| {
            assert!(
                type_args.is_empty(),
                "trait-level generics pass no type args"
            );
            match (f, args) {
                ("get", [ScriptValue::String(_)]) => {
                    Ok(ScriptValue::Optional(Some(Box::new(ScriptValue::I64(41)))))
                }
                ("put", [ScriptValue::String(_), ScriptValue::I64(_)]) => Ok(ScriptValue::Unit),
                ("refresh", []) => Ok(ScriptValue::I64(7)),
                other => panic!("unexpected call {other:?}"),
            }
        })));
    assert_eq!(ints.get("answer".into()), Some(41));
    ints.put("answer".into(), 42);
    assert_eq!(block_on(ints.refresh()).unwrap(), 7);

    let strings: StoreHandle<String> =
        StoreHandle::from_caller(Box::new(MapCaller(|f, _, args| match (f, args) {
            ("get", [ScriptValue::String(k)]) => Ok(ScriptValue::Optional(Some(Box::new(
                ScriptValue::String(format!("value of {k}")),
            )))),
            other => panic!("unexpected call {other:?}"),
        })));
    assert_eq!(strings.get("k".into()), Some("value of k".to_string()));
}

#[test]
fn generic_result_method_surfaces_conversion_errors() {
    let ints: StoreHandle<i32> = StoreHandle::from_caller(Box::new(MapCaller(|_, _, _| {
        Ok(ScriptValue::String("not a number".into()))
    })));
    let err = block_on(ints.refresh()).unwrap_err();
    assert!(err.0.contains("unexpected value"));
}

// ── Method-level generics: instantiations declared on the method, type
// arguments passed at dispatch time.

/// Converts host values.
#[script(foreign)]
pub trait Converter {
    #[script(instantiate(i64), instantiate(String))]
    fn convert<U>(&self, raw: String) -> U;
}

#[test]
fn method_generics_are_described_with_instantiations() {
    let desc = <ConverterHandle as ScriptForeign>::DESCRIPTOR;
    assert!(desc.generic_params.is_empty());
    let convert = &desc.functions[0];
    assert_eq!(convert.generic_params.len(), 1);
    assert_eq!(convert.generic_params[0].name, "U");
    assert_eq!(*convert.return_type, TypeDescriptor::GenericParam("U"));
    assert_eq!(
        convert.instantiations,
        &[
            &[TypeDescriptor::Primitive(PrimitiveType::I64)] as &[_],
            &[TypeDescriptor::String] as &[_],
        ]
    );
}

#[test]
fn method_generics_dispatch_with_type_args() {
    let handle = ConverterHandle::from_caller(Box::new(MapCaller(|f, type_args, args| {
        assert_eq!(f, "convert");
        let [ScriptValue::String(raw)] = args else {
            panic!("expected one string arg");
        };
        match type_args {
            [TypeDescriptor::Primitive(PrimitiveType::I64)] => {
                Ok(ScriptValue::I64(raw.parse().unwrap()))
            }
            [TypeDescriptor::String] => Ok(ScriptValue::String(format!("<{raw}>"))),
            other => panic!("unexpected type args {other:?}"),
        }
    })));
    let n: i64 = handle.convert("42".to_string());
    assert_eq!(n, 42);
    let s: String = handle.convert("42".to_string());
    assert_eq!(s, "<42>");
}

// ── Declared dispatch modes: `dyn` formalizes erased addressing (one host
// handler under the plain name); bare `dyn` gets the default candidate set.

/// Renders host values.
#[script(foreign)]
pub trait Render {
    #[script(dyn)]
    fn show<U>(&self, value: U) -> String;

    #[script(dyn, instantiate(i64), instantiate(bool))]
    fn tag<U>(&self, value: U) -> String;
}

#[test]
fn dyn_foreign_methods_declare_erased_dispatch() {
    use haphe::Dispatch;
    let desc = <RenderHandle as ScriptForeign>::DESCRIPTOR;
    let show = &desc.functions[0];
    assert_eq!(show.dispatch, Dispatch::Dyn);
    // Bare `dyn` on a single-parameter generic: default candidate set.
    assert_eq!(show.instantiations.len(), 5);
    let tag = &desc.functions[1];
    assert_eq!(tag.dispatch, Dispatch::Dyn);
    assert_eq!(tag.instantiations.len(), 2);
}

#[test]
fn dyn_foreign_calls_still_carry_type_args_as_data() {
    // The handle's wrapper is identical in both modes — the caller decides
    // what the declared dispatch means for its runtime.
    let handle = RenderHandle::from_caller(Box::new(MapCaller(|f, type_args, args| {
        assert_eq!(f, "show");
        assert_eq!(type_args.len(), 1);
        let [value] = args else { panic!("one arg") };
        Ok(ScriptValue::String(format!("{value:?}")))
    })));
    let shown: String = handle.show(7i64);
    assert!(shown.contains('7'));
}
