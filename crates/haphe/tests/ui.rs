//! Compile-fail tests pinning the error messages users see. Error text is an
//! API surface: review `.stderr` changes as carefully as code changes.

#![cfg(feature = "macros")]

#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
    // These fixtures pin stderr that embeds rustc's exhaustive impl listing,
    // which grows when optional integration features add bridge impls — so
    // they only run against the default impl set.
    #[cfg(not(any(
        feature = "bitflags",
        feature = "streams",
        feature = "futures",
        feature = "chrono",
        feature = "jiff"
    )))]
    t.compile_fail("tests/ui_impl_listing/*.rs");
    #[cfg(feature = "bitflags")]
    t.compile_fail("tests/ui_bitflags/*.rs");
}
