//! `#[script]` on free functions exposes a descriptor through the
//! `ScriptFunction` trait, reachable through imports and re-exports.

#![cfg(feature = "macros")]

use haphe::{
    FunctionDescriptor, Ownership, ParamDescriptor, PrimitiveType, ScriptFunction, TypeDescriptor,
    script,
};

/// Adds two numbers.
#[script]
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[script(rename = "shout", error_kind = "ValueError")]
fn make_loud(text: &str) -> String {
    text.to_uppercase()
}

pub mod math {
    use haphe::script;

    #[script]
    pub fn mul(a: f64, b: f64) -> f64 {
        a * b
    }
}

// Descriptors travel with imports and renamed re-exports.
use math::mul;
use math::mul as multiply;

/// A function with explicit (but monomorphic) lifetimes is exposable.
#[allow(clippy::needless_lifetimes)]
#[script]
pub fn first_word<'a>(s: &'a str) -> &'a str {
    s.split_whitespace().next().unwrap_or("")
}

/// Raw identifiers are exposed without the `r#` prefix.
#[script]
pub fn r#loop(n: i32) -> i32 {
    n
}

const I32: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::I32);

static EXPECTED_ADD: FunctionDescriptor<'static> = FunctionDescriptor {
    name: "add",
    doc: Some("Adds two numbers."),
    receiver: None,
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

/// Descriptors are usable in static initializers (`registry!` relies on this).
static IN_STATIC: &[FunctionDescriptor<'static>] = &[
    <add as ScriptFunction>::DESCRIPTOR,
    <math::mul as ScriptFunction>::DESCRIPTOR,
];

#[test]
fn descriptor_matches_hand_written() {
    assert_eq!(<add as ScriptFunction>::DESCRIPTOR, EXPECTED_ADD);
    assert_eq!(add(2, 3), 5);
}

#[test]
fn rename_and_error_kind() {
    let desc = <make_loud as ScriptFunction>::DESCRIPTOR;
    assert_eq!(desc.name, "shout");
    assert_eq!(desc.error_kind, Some("ValueError"));
    assert_eq!(desc.params[0].ownership, Ownership::Ref);
    assert_eq!(*desc.params[0].ty, TypeDescriptor::String);
    assert_eq!(make_loud("hey"), "HEY");
}

#[test]
fn imports_and_reexports_resolve() {
    assert_eq!(IN_STATIC.len(), 2);
    assert_eq!(IN_STATIC[1].name, "mul");
    assert_eq!(<mul as ScriptFunction>::DESCRIPTOR.name, "mul");
    assert_eq!(<multiply as ScriptFunction>::DESCRIPTOR.name, "mul");
    assert_eq!(math::mul(2.0, 4.0), 8.0);
}

#[test]
fn lifetimes_are_erased() {
    let desc = <first_word as ScriptFunction>::DESCRIPTOR;
    assert_eq!(*desc.params[0].ty, TypeDescriptor::String);
    assert_eq!(*desc.return_type, TypeDescriptor::String);
    assert_eq!(desc.return_ownership, Ownership::Ref);
    assert_eq!(first_word("hello world"), "hello");
}

#[test]
fn raw_identifiers_are_unrawed() {
    assert_eq!(<r#loop as ScriptFunction>::DESCRIPTOR.name, "loop");
    assert_eq!(r#loop(7), 7);
}
