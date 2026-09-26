//! Streams and futures are unconditionally unsupported: Rhai has no async
//! runtime, so registries carrying them are rejected at capability check.

#![allow(
    clippy::needless_pass_by_value,
    clippy::doc_markdown,
    reason = "fixture shapes are dictated by the bridge surface under test: `#[script]` functions receive OWNED values (the boundary contract) and docs name fixture idents verbatim"
)]
#![allow(
    dead_code,
    reason = "fixtures are exercised through their generated descriptors and bridge wrappers, not direct calls"
)]

use haphe::RuntimeBinder;
use haphe_rhai::RhaiBinder;

#[test]
fn capabilities_report_no_streams() {
    let caps = RhaiBinder::new().capabilities();
    assert!(!caps.streams);
}

#[test]
fn capabilities_report_no_futures() {
    let caps = RhaiBinder::new().capabilities();
    assert!(!caps.futures);
}
