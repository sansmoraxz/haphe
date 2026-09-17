//! `jiff` timestamp coverage (feature `jiff`): `Timestamp` crosses as a
//! `(secs: i64, nanos: i32)` tuple since the Unix epoch — jiff's own
//! constructor shape, where a fractional nanosecond carries the second's
//! sign.

use jiff::Timestamp;

use crate::bridge::{FromScript, ScriptConvertError, ScriptValue};
use crate::haphe_type::HapheType;
use crate::types::TypeDescriptor;

impl HapheType for Timestamp {
    const DESCRIPTOR: TypeDescriptor<'static> =
        TypeDescriptor::Tuple(&[TypeDescriptor::I64, TypeDescriptor::I32]);
}

impl From<Timestamp> for ScriptValue {
    fn from(t: Timestamp) -> Self {
        ScriptValue::from((t.as_second(), t.subsec_nanosecond()))
    }
}

impl FromScript for Timestamp {
    fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
        let (secs, nanos) = <(i64, i32)>::from_script(v).map_err(|e| ScriptConvertError {
            expected: "timestamp (secs, nanos) tuple",
            got: e.got,
        })?;
        Timestamp::new(secs, nanos).map_err(|_| ScriptConvertError {
            expected: "timestamp (secs, nanos) tuple",
            got: "timestamp out of range",
        })
    }
}
