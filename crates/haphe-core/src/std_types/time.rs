//! Time types cross as faithful tuples (mirroring `Duration::new`), never as
//! lossy float seconds.
//!
//! - `Duration` ↔ `(secs: u64, nanos: u32)`
//! - `SystemTime` ↔ `(secs: i64, nanos: u32)` since the Unix epoch, secs
//!   signed so pre-epoch times cross faithfully; nanos always in
//!   `0..1_000_000_000` (the instant is `epoch + secs + nanos`).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::bridge::{FromScript, ScriptConvertError, ScriptValue};
use crate::haphe_type::HapheType;
use crate::types::TypeDescriptor;

impl HapheType for Duration {
    const DESCRIPTOR: TypeDescriptor<'static> =
        TypeDescriptor::Tuple(&[TypeDescriptor::U64, TypeDescriptor::U32]);
}

impl From<Duration> for ScriptValue {
    fn from(d: Duration) -> Self {
        ScriptValue::from((d.as_secs(), d.subsec_nanos()))
    }
}

impl FromScript for Duration {
    fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
        let (secs, nanos) = <(u64, u32)>::from_script(v).map_err(|e| ScriptConvertError {
            expected: "duration (secs, nanos) tuple",
            got: e.got,
        })?;
        if nanos >= 1_000_000_000 {
            return Err(ScriptConvertError {
                expected: "duration (secs, nanos) tuple",
                got: "nanos out of range",
            });
        }
        Ok(Duration::new(secs, nanos))
    }
}

impl HapheType for SystemTime {
    const DESCRIPTOR: TypeDescriptor<'static> =
        TypeDescriptor::Tuple(&[TypeDescriptor::I64, TypeDescriptor::U32]);
}

impl From<SystemTime> for ScriptValue {
    fn from(t: SystemTime) -> Self {
        let (secs, nanos) = match t.duration_since(UNIX_EPOCH) {
            Ok(d) => (d.as_secs() as i64, d.subsec_nanos()),
            Err(e) => {
                let d = e.duration();
                if d.subsec_nanos() == 0 {
                    (-(d.as_secs() as i64), 0)
                } else {
                    (-(d.as_secs() as i64) - 1, 1_000_000_000 - d.subsec_nanos())
                }
            }
        };
        ScriptValue::from((secs, nanos))
    }
}

impl FromScript for SystemTime {
    fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
        let (secs, nanos) = <(i64, u32)>::from_script(v).map_err(|e| ScriptConvertError {
            expected: "timestamp (secs, nanos) tuple",
            got: e.got,
        })?;
        if nanos >= 1_000_000_000 {
            return Err(ScriptConvertError {
                expected: "timestamp (secs, nanos) tuple",
                got: "nanos out of range",
            });
        }
        let time = if secs >= 0 {
            UNIX_EPOCH.checked_add(Duration::new(secs as u64, nanos))
        } else if nanos == 0 {
            UNIX_EPOCH.checked_sub(Duration::new(secs.unsigned_abs(), 0))
        } else {
            UNIX_EPOCH.checked_sub(Duration::new(
                (secs + 1).unsigned_abs(),
                1_000_000_000 - nanos,
            ))
        };
        time.ok_or(ScriptConvertError {
            expected: "timestamp (secs, nanos) tuple",
            got: "timestamp out of range",
        })
    }
}
