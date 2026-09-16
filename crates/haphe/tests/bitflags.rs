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

#[test]
fn flags_record_real_bit_values_and_repr() {
    use haphe::{PrimitiveType, ScriptEnum};
    let desc = <Perms as ScriptEnum>::DESCRIPTOR;
    assert_eq!(desc.repr, Some(PrimitiveType::U32));
    let bits: Vec<Option<i64>> = desc.variants.iter().map(|v| v.discriminant).collect();
    // Real bit VALUES, not declaration indices.
    assert!(bits.iter().all(|b| b.is_some()));
    for (i, b) in bits.iter().enumerate() {
        let v = b.unwrap();
        assert!(v != 0 && (v & (v - 1)) == 0, "variant {i} is a single bit");
    }
}
