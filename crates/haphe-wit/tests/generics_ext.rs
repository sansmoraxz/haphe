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

// ---------------------------------------------------------------------------
// Registry-level instantiations (no fn-site `instantiate` attrs needed)
// ---------------------------------------------------------------------------

/// Relays a value. Deliberately carries NO `instantiate(...)` attributes:
/// the registry alone declares the concrete uses.
#[script]
fn relay<T>(value: T) -> T {
    value
}

haphe::registry! {
    pub static REGISTRY_LEVEL = {
        modules: [
            mod pipes { functions: [relay<i64>, relay<String>] },
        ],
    };
}

// Mixed sources on `echo`: fn-site declares i64 + String; the registry
// repeats i64 (dedupe) and adds bool (union).
haphe::registry! {
    pub static MIXED = {
        modules: [
            mod util { functions: [echo<i64>, echo<bool>] },
        ],
    };
}

fn generate_from(registry: &'static haphe::TypeRegistry<'static>) -> String {
    let output =
        haphe::generate(&WitGenerator::new("haphe:demo"), registry).expect("generation succeeds");
    String::from_utf8(output.files[0].content.clone()).unwrap()
}

/// WIT artifacts generate from registry-level instantiations alone,
/// with the same descriptor-derived mangled names the
/// fn-site attributes would produce.
#[test]
fn registry_only_instantiations_emit_monomorphs() {
    let wit = generate_from(&REGISTRY_LEVEL);
    assert!(
        wit.contains("/// haphe:generic-instance = relay<s64>"),
        "got:\n{wit}"
    );
    assert!(
        wit.contains("relay-s64: func(value: s64) -> s64;"),
        "got:\n{wit}"
    );
    assert!(
        wit.contains("relay-string: func(value: string) -> string;"),
        "got:\n{wit}"
    );
}

/// An instantiation declared at both sources emits exactly one monomorph
/// (no name collision); a registry-only extra emits alongside the fn-site
/// ones.
#[test]
fn mixed_sources_dedupe_and_union() {
    let wit = generate_from(&MIXED);
    assert_eq!(wit.matches("echo-s64: func").count(), 1, "got:\n{wit}");
    assert!(
        wit.contains("echo-string: func(value: string) -> string;"),
        "got:\n{wit}"
    );
    assert!(
        wit.contains("echo-bool: func(value: bool) -> bool;"),
        "got:\n{wit}"
    );
}

// Silence unused-type shims the traits need.
struct Unused;

impl From<ForeignError> for Unused {
    fn from(_: ForeignError) -> Self {
        Unused
    }
}
