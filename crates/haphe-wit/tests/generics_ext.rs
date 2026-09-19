//! The `generics` naming extension: monomorphized generic functions and
//! foreign interfaces, emitted under deterministic descriptor-derived names.

#![cfg(feature = "generics")]
#![allow(
    dead_code,
    clippy::needless_pass_by_value,
    clippy::unused_self,
    reason = "fixture shapes are dictated by the bridge surface under test: receivers and owned parameters mirror the declared script signatures, not local call ergonomics"
)]

use haphe::{ForeignError, Script, script};
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

// ---------------------------------------------------------------------------
// Static generic METHOD monomorphs: resource members and enum companions
// under the same mangled names as generic free functions.
// ---------------------------------------------------------------------------

/// Holds a running total.
#[derive(Script, Clone)]
#[script(thread_safety = send_sync, methods)]
struct Holder {
    total: i64,
}

#[script]
impl Holder {
    /// Picks the first of two values.
    #[script(instantiate(i64), instantiate(String))]
    fn first_of<T>(&self, a: T, b: T) -> T {
        let _ = b;
        a
    }

    /// Accumulates a value.
    #[script(instantiate(i64))]
    fn add_in<T: Into<i64>>(&mut self, value: T) {
        self.total += value.into();
    }
}

/// Selection mode.
#[derive(Script, Clone)]
#[script(thread_safety = send_sync, methods)]
enum Mode {
    First,
    Second,
}

#[script]
impl Mode {
    /// Picks by mode.
    #[script(instantiate(bool))]
    fn pick<T>(&self, a: T, b: T) -> T {
        match self {
            Mode::First => a,
            Mode::Second => b,
        }
    }
}

haphe::registry! {
    pub static METHOD_MONOMORPHS = {
        structs: [Holder],
        enums: [Mode],
    };
}

#[test]
fn generic_method_monomorphs_are_emitted() {
    let wit = generate_from(&METHOD_MONOMORPHS);
    assert!(
        wit.contains("/// haphe:generic-instance = first_of<s64>"),
        "got:\n{wit}"
    );
    assert!(
        wit.contains("first-of-s64: func(a: s64, b: s64) -> s64;"),
        "got:\n{wit}"
    );
    assert!(
        wit.contains("first-of-string: func(a: string, b: string) -> string;"),
        "got:\n{wit}"
    );
    assert!(wit.contains("add-in-s64: func(value: s64);"), "got:\n{wit}");
    // The un-mangled generic signatures never appear.
    assert!(!wit.contains("first-of: func"), "got:\n{wit}");
}

#[test]
fn generic_enum_method_monomorphs_are_companions() {
    let wit = generate_from(&METHOD_MONOMORPHS);
    assert!(
        wit.contains("mode-pick-bool: func(this: mode, a: bool, b: bool) -> bool;"),
        "got:\n{wit}"
    );
}

// Async generic methods emit monomorphs too (async is a runtime lifting
// concern; the text is dispatch- and asyncness-agnostic apart from the
// keyword). Hand-built: the descriptor is all the generator consumes.
static ASYNC_GENERIC_METHOD: [haphe::FunctionDescriptor; 1] = [haphe::FunctionDescriptor {
    name: "fetch",
    doc: None,
    receiver: Some(haphe::Receiver::Ref),
    generic_params: &[haphe::GenericParam {
        name: "T",
        bounds: &[],
        default: None,
    }],
    instantiations: &[&[haphe::TypeDescriptor::I64]],
    dispatch: haphe::Dispatch::Static,
    params: &[haphe::ParamDescriptor {
        name: "key",
        ty: &haphe::TypeDescriptor::GenericParam("T"),
        ownership: haphe::Ownership::Owned,
    }],
    return_type: &haphe::TypeDescriptor::GenericParam("T"),
    return_ownership: haphe::Ownership::Owned,
    is_async: true,
    error_kind: None,
    fallible: false,
}];

static ASYNC_HOLDER: [haphe::StructDescriptor; 1] = [haphe::StructDescriptor {
    id: haphe::TypeId::new("test::AsyncHolder"),
    name: "AsyncHolder",
    doc: None,
    fields: &[],
    methods: &ASYNC_GENERIC_METHOD,
    constructors: &[],
    properties: &[],
    trait_impls: &[],
    thread_safety: haphe::ThreadSafety::SEND_SYNC,
    generic_params: &[],
}];

static ASYNC_REGISTRY: haphe::TypeRegistry =
    haphe::TypeRegistry::new(&ASYNC_HOLDER, &[], &[], &[], &[], &[]);

#[test]
fn async_generic_method_monomorphs_are_emitted() {
    let wit = generate_from(&ASYNC_REGISTRY);
    assert!(
        wit.contains("fetch-s64: async func(key: s64) -> s64;"),
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
