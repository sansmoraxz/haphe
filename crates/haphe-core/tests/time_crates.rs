//! Feature-gated bridge coverage for third-party time crates (`chrono`,
//! `jiff`), mirroring the `chrono_support`/`jiff_support` modules.

#![cfg(any(feature = "chrono", feature = "jiff"))]

use haphe_core::bridge::{FromScript, ScriptValue};

fn roundtrip<T>(value: T) -> T
where
    T: FromScript + Clone,
    ScriptValue: From<T>,
{
    T::from_script(ScriptValue::from(value)).unwrap()
}

#[cfg(feature = "chrono")]
#[test]
fn chrono_datetime_crosses_as_timestamp_tuple() {
    use chrono::{DateTime, Utc};
    let t: DateTime<Utc> = DateTime::from_timestamp(1_700_000_000, 123).unwrap();
    assert_eq!(roundtrip(t), t);
    let err = DateTime::<Utc>::from_script(ScriptValue::from((i64::MAX, 0u32))).unwrap_err();
    assert_eq!(err.got, "timestamp out of range");
}

#[cfg(feature = "jiff")]
#[test]
fn jiff_timestamp_crosses_as_timestamp_tuple() {
    use jiff::Timestamp;
    let t = Timestamp::new(1_700_000_000, 123).unwrap();
    assert_eq!(roundtrip(t), t);
    let err = Timestamp::from_script(ScriptValue::from((i64::MAX, 0i32))).unwrap_err();
    assert_eq!(err.got, "timestamp out of range");
}
