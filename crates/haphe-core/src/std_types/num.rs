//! Numeric coverage beyond the fixed-width primitives: pointer-width
//! integers, 128-bit integers (script→Rust only), and `NonZero*`.

use crate::bridge::{FromScript, ScriptConvertError, ScriptValue};
use crate::haphe_type::HapheType;
use crate::types::TypeDescriptor;

// Pointer-width integers cross like their fixed-width descriptors (`usize`
// as `u64`, `isize` as `i64`): the bridge carries the `i64` bit pattern,
// matching the fixed-width integer conversions.
impl From<usize> for ScriptValue {
    fn from(n: usize) -> Self {
        Self::I64(n as i64)
    }
}

impl FromScript for usize {
    fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
        match v {
            ScriptValue::I64(n) => Ok(n as usize),
            other => Err(ScriptConvertError {
                expected: "usize",
                got: other.variant_name(),
            }),
        }
    }
}

impl From<isize> for ScriptValue {
    fn from(n: isize) -> Self {
        Self::I64(n as i64)
    }
}

impl FromScript for isize {
    fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
        match v {
            ScriptValue::I64(n) => Ok(n as isize),
            other => Err(ScriptConvertError {
                expected: "isize",
                got: other.variant_name(),
            }),
        }
    }
}

// 128-bit integers convert script→Rust only: the carried `i64` widens
// losslessly, but a Rust `i128`/`u128` value may not fit 64 bits and the
// outbound conversion is infallible, so it is deliberately absent — a
// 128-bit return type stays descriptor-only rather than silently
// truncating.
impl FromScript for i128 {
    fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
        match v {
            ScriptValue::I64(n) => Ok(i128::from(n)),
            other => Err(ScriptConvertError {
                expected: "i128",
                got: other.variant_name(),
            }),
        }
    }
}

impl FromScript for u128 {
    fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
        match v {
            ScriptValue::I64(n) => u128::try_from(n).map_err(|_| ScriptConvertError {
                expected: "u128",
                got: "negative integer",
            }),
            other => Err(ScriptConvertError {
                expected: "u128",
                got: other.variant_name(),
            }),
        }
    }
}

macro_rules! impl_non_zero {
    ($($nz:ident => $inner:ty),* $(,)?) => {$(
        /// Described as its inner integer; the script→Rust conversion
        /// rejects zero.
        impl HapheType for core::num::$nz {
            const DESCRIPTOR: TypeDescriptor<'static> = <$inner as HapheType>::DESCRIPTOR;
        }

        impl From<core::num::$nz> for ScriptValue {
            fn from(n: core::num::$nz) -> Self {
                ScriptValue::from(n.get())
            }
        }

        impl FromScript for core::num::$nz {
            fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
                let inner = <$inner as FromScript>::from_script(v).map_err(|e| {
                    ScriptConvertError {
                        expected: stringify!($nz),
                        got: e.got,
                    }
                })?;
                Self::new(inner).ok_or(ScriptConvertError {
                    expected: stringify!($nz),
                    got: "zero",
                })
            }
        }
    )*};
}

impl_non_zero! {
    NonZeroI8 => i8,
    NonZeroI16 => i16,
    NonZeroI32 => i32,
    NonZeroI64 => i64,
    NonZeroIsize => isize,
    NonZeroU8 => u8,
    NonZeroU16 => u16,
    NonZeroU32 => u32,
    NonZeroU64 => u64,
    NonZeroUsize => usize,
}
