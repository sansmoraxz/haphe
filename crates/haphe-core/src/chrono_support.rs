//! `chrono` timestamp coverage (feature `chrono`): `DateTime<Utc>` crosses
//! as a `(secs: i64, nanos: u32)` tuple since the Unix epoch, the same shape
//! as `SystemTime`.

use chrono::{DateTime, Utc};

use crate::bridge::{FromScript, ScriptConvertError, ScriptValue};
use crate::haphe_type::HapheType;
use crate::types::TypeDescriptor;

impl HapheType for DateTime<Utc> {
    const DESCRIPTOR: TypeDescriptor<'static> =
        TypeDescriptor::Tuple(&[TypeDescriptor::I64, TypeDescriptor::U32]);
}

impl From<DateTime<Utc>> for ScriptValue {
    fn from(t: DateTime<Utc>) -> Self {
        ScriptValue::from((t.timestamp(), t.timestamp_subsec_nanos()))
    }
}

impl FromScript for DateTime<Utc> {
    fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
        let (secs, nanos) = <(i64, u32)>::from_script(v).map_err(|e| ScriptConvertError {
            expected: "timestamp (secs, nanos) tuple",
            got: e.got,
        })?;
        DateTime::from_timestamp(secs, nanos).ok_or(ScriptConvertError {
            expected: "timestamp (secs, nanos) tuple",
            got: "timestamp out of range",
        })
    }
}
