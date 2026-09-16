//! Adoption of [`bitflags`](https://docs.rs/bitflags)-crate types as haphe
//! flags enums, behind the `bitflags` cargo feature.
//!
//! [`script_bitflags!`](crate::script_bitflags) builds an
//! [`EnumDescriptor`](crate::EnumDescriptor) with `is_flags: true` at compile
//! time from the type's own `bitflags::Flags::FLAGS` metadata, so flag names
//! never drift from the source definition.

/// Exposes a [`bitflags`]-defined type to haphe as a flags enum.
///
/// Each named flag becomes one unit variant, in declaration order. Every
/// named flag must set **exactly one bit** — a composite mask (e.g.
/// `ALL = READ | WRITE`) or an empty mask fails at compile time, because
/// backend bitset formats model independent single bits only.
///
/// ```
/// haphe_core::bitflags::bitflags! {
///     #[derive(Clone, Copy)]
///     pub struct Perms: u32 {
///         const READ = 1;
///         const WRITE = 2;
///         const EXEC = 4;
///     }
/// }
/// haphe_core::script_bitflags!(Perms);
///
/// use haphe_core::ScriptEnum;
/// assert!(Perms::DESCRIPTOR.is_flags);
/// assert_eq!(Perms::DESCRIPTOR.variants.len(), 3);
/// assert_eq!(Perms::DESCRIPTOR.variants[0].name, "READ");
/// ```
/// Maps a bitflags `Bits` width to the propagated unsigned primitive.
#[doc(hidden)]
pub const fn bitflags_repr(size: usize) -> crate::types::PrimitiveType {
    match size {
        1 => crate::types::PrimitiveType::U8,
        2 => crate::types::PrimitiveType::U16,
        4 => crate::types::PrimitiveType::U32,
        8 => crate::types::PrimitiveType::U64,
        _ => panic!("script_bitflags!: unsupported flags width"),
    }
}

#[macro_export]
macro_rules! script_bitflags {
    ($ty:ty) => {
        const _: () = {
            const FLAGS: &[$crate::bitflags::Flag<$ty>] =
                <$ty as $crate::bitflags::Flags>::FLAGS;
            const N: usize = FLAGS.len();

            const VARIANTS: [$crate::EnumVariant<'static>; N] = {
                let mut out = [$crate::EnumVariant {
                    name: "",
                    doc: None,
                    kind: $crate::VariantKind::Unit,
                    discriminant: None,
                }; N];
                let mut i = 0;
                while i < N {
                    let bits = FLAGS[i].value().bits();
                    assert!(
                        bits != 0 && (bits & (bits - 1)) == 0,
                        "script_bitflags!: every named flag must set exactly one bit; \
                         composite or empty masks cannot be represented as independent flags"
                    );
                    // Real bit VALUE (not declaration order) — stored as the
                    // i64 bit pattern; flags wider than 64 bits are rejected.
                    let value = bits as u128;
                    assert!(
                        value <= u64::MAX as u128,
                        "script_bitflags!: flag bits beyond 64 bits cannot be represented"
                    );
                    out[i] = $crate::EnumVariant {
                        name: FLAGS[i].name(),
                        doc: None,
                        kind: $crate::VariantKind::Unit,
                        discriminant: Some(value as u64 as i64),
                    };
                    i += 1;
                }
                out
            };

            impl $crate::ScriptEnum for $ty {
                const DESCRIPTOR: $crate::EnumDescriptor<'static> = $crate::EnumDescriptor {
                    id: <$ty as $crate::ScriptType>::ID,
                    name: ::core::stringify!($ty),
                    doc: None,
                    variants: &VARIANTS,
                    methods: &[],
                    trait_impls: &[],
                    thread_safety: $crate::ThreadSafety::SEND_SYNC,
                    generic_params: &[],
                    repr: Some($crate::bitflags_repr(::core::mem::size_of::<
                        <$ty as $crate::bitflags::Flags>::Bits,
                    >())),
                    is_flags: true,
                };
            }
        };

        impl $crate::ScriptType for $ty {
            const ID: $crate::TypeId<'static> = $crate::TypeId::new(::core::concat!(
                ::core::module_path!(),
                "::",
                ::core::stringify!($ty)
            ));
        }

        impl $crate::HapheType for $ty {
            const DESCRIPTOR: $crate::TypeDescriptor<'static> =
                $crate::TypeDescriptor::Ref(<$ty as $crate::ScriptType>::ID);
        }
    };
}
