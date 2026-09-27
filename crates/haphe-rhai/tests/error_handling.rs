//! Error variant coverage for `RhaiBindError`: async rejection and
//! Display output.

#![allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    clippy::doc_markdown,
    reason = "fixture shapes are dictated by the bridge surface under test: `#[script]` functions receive OWNED values (the boundary contract), methods keep their declared receivers (`&self` on Copy enums included), and docs name fixture idents verbatim"
)]
#![allow(
    dead_code,
    reason = "fixtures are exercised through their generated descriptors and bridge wrappers, not direct calls"
)]

use haphe::{Script, script};
use haphe_rhai::{RhaiBindError, bind_type};

#[derive(Script, Clone)]
#[script(thread_safety = none, methods)]
struct AsyncType {
    value: i64,
}

#[script]
impl AsyncType {
    #[script(constructor)]
    fn new(value: i64) -> Self {
        AsyncType { value }
    }

    async fn fetch(&self) -> String {
        format!("fetched {}", self.value)
    }
}

#[test]
fn async_method_rejected() {
    let mut engine = rhai::Engine::new();
    let mut type_mod = rhai::Module::new();
    let err = bind_type::<AsyncType>(&mut engine, &mut type_mod).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("async") && msg.contains("fetch"),
        "expected async rejection mentioning `fetch`, got: {msg}"
    );
    assert!(
        matches!(err, RhaiBindError::UnsupportedAsyncMethod { name: "fetch" }),
        "got: {err}"
    );
}

#[test]
fn error_display_messages_are_descriptive() {
    let variants: Vec<RhaiBindError> = vec![
        RhaiBindError::UnsupportedAsyncMethod { name: "test" },
        RhaiBindError::UnsupportedAsyncFunction { name: "test" },
        RhaiBindError::UnsupportedAsyncProperty { name: "test" },
        RhaiBindError::UnsupportedAsyncCall,
        RhaiBindError::GenericFunction { name: "test" },
        RhaiBindError::MissingForeignFunction {
            interface: "A",
            function: "b",
        },
        RhaiBindError::GenericForeignInterface { name: "A" },
        RhaiBindError::ReservedMethod { name: "test" },
        RhaiBindError::DuplicateConstructor { name: "new" },
        RhaiBindError::DuplicateMethod {
            name: "test".into(),
        },
        RhaiBindError::DuplicateField { name: "x" },
        RhaiBindError::InvalidConstant {
            module: "m".into(),
            name: "C".into(),
            value: "???".into(),
        },
    ];
    for variant in &variants {
        let msg = variant.to_string();
        assert!(
            !msg.is_empty(),
            "empty Display for {variant:?}"
        );
    }
}
