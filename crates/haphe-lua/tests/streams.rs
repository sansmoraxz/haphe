//! Stream/future types pass through the Lua binder at stub level: functions
//! carrying them bind as stubs and the binder stays capability-compatible.

#![allow(dead_code)]

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
    assert!(err.to_string().contains("not yet implemented"));
}
