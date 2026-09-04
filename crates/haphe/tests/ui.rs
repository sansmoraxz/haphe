//! Compile-fail tests pinning the error messages users see. Error text is an
//! API surface: review `.stderr` changes as carefully as code changes.

#![cfg(feature = "macros")]

#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
