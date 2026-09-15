//! The `bitflags`-crate adapter: `script_bitflags!` builds a flags
//! `EnumDescriptor` from the type's own `Flags::FLAGS` metadata.

#![cfg(feature = "bitflags")]

use haphe::ScriptEnum;

haphe::bitflags::bitflags! {
    #[derive(Clone, Copy)]
    pub struct Perms: u32 {
        const READ = 1;
        const WRITE = 2;
        const EXEC = 4;
    }
}

haphe::script_bitflags!(Perms);

haphe::registry! {
    pub static REGISTRY = {
        enums: [Perms],
    };
}

#[test]
fn adapter_descriptor() {
    let desc = <Perms as ScriptEnum>::DESCRIPTOR;
    assert!(desc.is_flags);
    let names: Vec<&str> = desc.variants.iter().map(|v| v.name).collect();
    assert_eq!(names, ["READ", "WRITE", "EXEC"]);
    assert!(
        desc.variants
            .iter()
            .all(|v| matches!(v.kind, haphe::VariantKind::Unit))
    );
}

#[test]
fn adapter_registry_validates() {
    REGISTRY.validate().expect("flags enum validates");
}
