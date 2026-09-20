//! Bridge coverage for commonly used standard-library types, grouped by
//! domain. Each file supplies [`HapheType`](crate::HapheType) descriptors
//! and/or [`FromScript`](crate::FromScript) / `From<T> for ScriptValue`
//! conversions so these types cross the bridge as plain values.
//!
//! Baseline primitive, string, container, tuple, and `Option` coverage lives
//! with the trait definitions in [`bridge`](crate::bridge) and
//! [`haphe_type`](crate::haphe_type); this module extends it.

mod collections;
mod num;
mod path;
mod pointers;
mod time;

mod net;
