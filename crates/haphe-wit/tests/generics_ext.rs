//! The `generics` naming extension: monomorphized generic functions and
//! foreign interfaces, emitted under deterministic descriptor-derived names.

#![cfg(feature = "generics")]
#![allow(dead_code)]

use haphe::{ForeignError, script};
use haphe_wit::WitGenerator;

/// Echoes a value.
#[script(instantiate(i64), instantiate(String))]
fn echo<T>(value: T) -> T {
    value
}

/// A host-backed store.
#[script(foreign, thread_safety = none)]
pub trait KvStore<T> {
    fn scale(&self, by: i64) -> T;
}

/// Converts raw host values.
#[script(foreign)]
pub trait Conv {
    #[script(instantiate(i64), instantiate(String))]
    fn convert<U>(&self, raw: i64) -> U;
}

haphe::registry! {
    pub static REGISTRY = {
        foreign: [KvStoreHandle<i64>, ConvHandle],
        modules: [
            mod util { functions: [echo] },
        ],
    };
}

fn generate() -> String {
    let output =
        haphe::generate(&WitGenerator::new("haphe:demo"), &REGISTRY).expect("generation succeeds");
    String::from_utf8(output.files[0].content.clone()).unwrap()
}

#[test]
fn generic_function_monomorphs_are_emitted() {
    let wit = generate();
    assert!(
        wit.contains("/// haphe:generic-instance = echo<s64>"),
        "got:\n{wit}"
    );
    assert!(
        wit.contains("echo-s64: func(value: s64) -> s64;"),
        "got:\n{wit}"
    );
    assert!(
        wit.contains("echo-string: func(value: string) -> string;"),
        "got:\n{wit}"
    );
}

#[test]
fn generic_foreign_interface_monomorphs_are_emitted() {
    let wit = generate();
    assert!(
        wit.contains("/// haphe:generic-instance = kv-store<s64>"),
        "got:\n{wit}"
    );
    assert!(wit.contains("interface kv-store-s64 {"), "got:\n{wit}");
    assert!(wit.contains("scale: func(by: s64) -> s64;"), "got:\n{wit}");
    assert!(wit.contains("export kv-store-s64;"), "got:\n{wit}");
}

#[test]
fn generic_foreign_method_monomorphs_are_emitted() {
    let wit = generate();
    assert!(wit.contains("interface conv {"), "got:\n{wit}");
    assert!(
        wit.contains("convert-s64: func(raw: s64) -> s64;"),
        "got:\n{wit}"
    );
    assert!(
        wit.contains("convert-string: func(raw: s64) -> string;"),
        "got:\n{wit}"
    );
    assert!(wit.contains("export conv;"), "got:\n{wit}");
}

// Silence unused-type shims the traits need.
struct Unused;

impl From<ForeignError> for Unused {
    fn from(_: ForeignError) -> Self {
        Unused
    }
}
