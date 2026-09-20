//! Stream/future types pass through the Lua binder at stub level: functions
//! carrying them bind as stubs and the binder stays capability-compatible.

#![allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    clippy::doc_markdown,
    clippy::trivially_copy_pass_by_ref,
    reason = "fixture shapes are dictated by the bridge surface under test: `#[script]` functions receive OWNED values (the boundary contract), methods keep their declared receivers (`&self` on Copy enums included), and docs name fixture idents verbatim"
)]
#![allow(
    dead_code,
    reason = "fixtures are exercised through their generated descriptors and bridge wrappers, not direct calls"
)]

use haphe::{RuntimeBinder, script};
use haphe_lua::LuaBinder;
use mlua::Lua;

/// Subscribes to a topic.
#[script]
fn subscribe(topic: String) -> haphe::Stream<String> {
    let _ = topic;
    unimplemented!()
}

haphe::registry! {
    pub static REGISTRY = {
        modules: [
            mod events {
                doc: "Event streaming",
                functions: [subscribe],
            },
        ],
    };
}

#[test]
fn binder_capabilities_accept_streams() {
    let validated = REGISTRY.validate().unwrap();
    LuaBinder::new()
        .capabilities()
        .check(&validated)
        .expect("streams allowed by LuaBinder's ALL capabilities");
}

#[test]
fn stream_fn_binds_as_stub() {
    let mut lua = Lua::new();
    let validated = REGISTRY.validate().unwrap();
    LuaBinder::new()
        .bind(&validated, &mut lua)
        .expect("binding succeeds");

    let err = lua
        .load("events.subscribe('news')")
        .exec()
        .expect_err("stub should error");
    assert!(err.to_string().contains("not bound"));
}
