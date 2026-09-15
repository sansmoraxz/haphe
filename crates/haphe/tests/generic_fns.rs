//! Generic free functions in the export direction: `instantiate(...)`
//! declarations become descriptor instantiations, and `ScriptBindFn`
//! registers one monomorphized wrapper per instantiation.

#![cfg(feature = "macros")]

use haphe::{
    BackendCapabilities, CompatibilityError, FnBinder, PrimitiveType, ScriptBindFn,
    ScriptConvertError, ScriptFunction, ScriptValue, TypeDescriptor, script,
};

/// Echoes a value.
#[script(instantiate(i64), instantiate(String))]
pub fn echo<T>(value: T) -> T {
    value
}

/// Declared generic but never instantiated.
#[script]
pub fn lonely<T>(value: T) -> T {
    value
}

haphe::registry! {
    pub static REGISTRY = {
        modules: [
            mod util { functions: [echo] },
        ],
    };
}

haphe::registry! {
    pub static LONELY_REGISTRY = {
        modules: [
            mod util { functions: [lonely] },
        ],
    };
}

#[test]
fn descriptor_records_params_and_instantiations() {
    let desc = <echo as ScriptFunction>::DESCRIPTOR;
    assert_eq!(desc.generic_params.len(), 1);
    assert_eq!(desc.generic_params[0].name, "T");
    assert_eq!(*desc.return_type, TypeDescriptor::GenericParam("T"));
    assert_eq!(
        desc.instantiations,
        &[
            &[TypeDescriptor::Primitive(PrimitiveType::I64)] as &[_],
            &[TypeDescriptor::String] as &[_],
        ]
    );
}

#[test]
fn registry_validates_and_capabilities_gate() {
    let validated = REGISTRY.validate().expect("registry validates");
    BackendCapabilities::ALL
        .check(&validated)
        .expect("instantiated generic fn is compatible");

    let errors = BackendCapabilities::ALL
        .with_generics(false)
        .check(&validated)
        .unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        CompatibilityError::UnsupportedModuleGenerics {
            module: "util",
            fn_name: "echo"
        }
    )));

    let validated = LONELY_REGISTRY.validate().expect("registry validates");
    let errors = BackendCapabilities::ALL.check(&validated).unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        CompatibilityError::UninstantiatedModuleGeneric {
            module: "util",
            fn_name: "lonely"
        }
    )));
}

type Wrapper = fn(&[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>;

#[derive(Default)]
struct Collect(Vec<(&'static str, &'static [TypeDescriptor<'static>], Wrapper)>);

impl FnBinder for Collect {
    type Error = std::convert::Infallible;

    fn function(
        &mut self,
        name: &'static str,
        type_args: &'static [TypeDescriptor<'static>],
        f: Wrapper,
    ) -> Result<(), Self::Error> {
        self.0.push((name, type_args, f));
        Ok(())
    }
}

#[test]
fn bind_registers_one_wrapper_per_instantiation() {
    let mut binder = Collect::default();
    <echo as ScriptBindFn>::bind(&mut binder).unwrap();
    let [(n1, t1, f1), (n2, t2, f2)] = binder.0.as_slice() else {
        panic!("expected two registrations, got {}", binder.0.len());
    };
    assert_eq!((*n1, *n2), ("echo", "echo"));
    assert_eq!(*t1, &[TypeDescriptor::Primitive(PrimitiveType::I64)]);
    assert_eq!(*t2, &[TypeDescriptor::String]);

    let out = f1(&[ScriptValue::I64(7)]).unwrap();
    assert!(matches!(out, ScriptValue::I64(7)));
    let out = f2(&[ScriptValue::String("hi".into())]).unwrap();
    assert!(matches!(out, ScriptValue::String(s) if s == "hi"));
    // The i64 wrapper rejects a string argument.
    assert!(f1(&[ScriptValue::String("hi".into())]).is_err());
}
