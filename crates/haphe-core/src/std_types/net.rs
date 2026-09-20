//! Network address types cross as their canonical display strings; the
//! script→Rust conversion parses and rejects malformed addresses
//! descriptively.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

use crate::bridge::{FromScript, ScriptConvertError, ScriptValue};
use crate::haphe_type::HapheType;
use crate::types::TypeDescriptor;

macro_rules! impl_addr {
    ($($ty:ident),* $(,)?) => {$(
        impl HapheType for $ty {
            const DESCRIPTOR: TypeDescriptor<'static> = TypeDescriptor::String;
        }

        impl From<$ty> for ScriptValue {
            fn from(addr: $ty) -> Self {
                Self::String(addr.to_string())
            }
        }

        impl FromScript for $ty {
            fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
                match v {
                    ScriptValue::String(s) => s.parse().map_err(|_| ScriptConvertError {
                        expected: stringify!($ty),
                        got: "malformed address string",
                    }),
                    other => Err(ScriptConvertError {
                        expected: stringify!($ty),
                        got: other.variant_name(),
                    }),
                }
            }
        }
    )*};
}

impl_addr!(
    IpAddr,
    Ipv4Addr,
    Ipv6Addr,
    SocketAddr,
    SocketAddrV4,
    SocketAddrV6
);
