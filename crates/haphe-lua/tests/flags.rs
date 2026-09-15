//! Flags enums (`#[script(flags)]` and `script_bitflags!`-adopted types)
//! pass through the Lua binder unchanged: the binder is agnostic to
//! `is_flags` and binds the usual type stubs.

#![allow(dead_code)]

use haphe::{RuntimeBinder, Script};
use haphe_lua::LuaBinder;
use mlua::Lua;

/// File permission bits.
#[derive(Script)]
#[script(flags)]
enum Perm {
    Read,
    Write,
    Exec,
}

haphe::bitflags::bitflags! {
    #[derive(Clone, Copy)]
    pub struct Caps: u32 {
        const NET = 1;
        const FS = 2;
    }
}

haphe::script_bitflags!(Caps);

haphe::registry! {
    pub static REGISTRY = {
        enums: [Perm, Caps],
        modules: [
            mod sys {
                doc: "System capabilities",
                types: [Perm, Caps],
            },
        ],
    };
}

#[test]
fn registry_with_flags_validates() {
    REGISTRY.validate().expect("flags enums validate");
}

#[test]
fn binder_capabilities_accept_flags() {
    let validated = REGISTRY.validate().unwrap();
    LuaBinder::new()
        .capabilities()
        .check(&validated)
        .expect("flags enums are capability-neutral for lua");
}

#[test]
fn flags_enums_bind_as_type_stubs() {
    let mut lua = Lua::new();
    let validated = REGISTRY.validate().unwrap();
    LuaBinder::new()
        .bind(&validated, &mut lua)
        .expect("binding succeeds");

    let sys: mlua::Table = lua.globals().get("sys").expect("sys module");
    let _perm: mlua::Table = sys.get("Perm").expect("Perm stub");
    let _caps: mlua::Table = sys.get("Caps").expect("Caps stub");
}
