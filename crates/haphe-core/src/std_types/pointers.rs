//! Smart-pointer and borrowed-form conversions: the pointer is erased at the
//! boundary and the inner value crosses. Descriptors already delegate (see
//! [`haphe_type`](crate::haphe_type)); these supply the value conversions.

use std::borrow::Cow;
use std::rc::Rc;
use std::sync::Arc;

use crate::bridge::{FromScript, ScriptConvertError, ScriptValue};

impl<T> From<Box<T>> for ScriptValue
where
    ScriptValue: From<T>,
{
    fn from(boxed: Box<T>) -> Self {
        ScriptValue::from(*boxed)
    }
}

impl<T: FromScript> FromScript for Box<T> {
    fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
        Ok(Box::new(T::from_script(v)?))
    }
}

/// Unwraps when this is the last reference, clones otherwise.
impl<T: Clone> From<Rc<T>> for ScriptValue
where
    ScriptValue: From<T>,
{
    fn from(rc: Rc<T>) -> Self {
        ScriptValue::from(Rc::try_unwrap(rc).unwrap_or_else(|rc| (*rc).clone()))
    }
}

impl<T: FromScript> FromScript for Rc<T> {
    fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
        Ok(Rc::new(T::from_script(v)?))
    }
}

/// Unwraps when this is the last reference, clones otherwise.
impl<T: Clone> From<Arc<T>> for ScriptValue
where
    ScriptValue: From<T>,
{
    fn from(arc: Arc<T>) -> Self {
        ScriptValue::from(Arc::try_unwrap(arc).unwrap_or_else(|arc| (*arc).clone()))
    }
}

impl<T: FromScript> FromScript for Arc<T> {
    fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
        Ok(Arc::new(T::from_script(v)?))
    }
}

impl<T> From<Cow<'_, T>> for ScriptValue
where
    T: ToOwned + ?Sized,
    ScriptValue: From<T::Owned>,
{
    fn from(cow: Cow<'_, T>) -> Self {
        ScriptValue::from(cow.into_owned())
    }
}

impl<T> FromScript for Cow<'_, T>
where
    T: ToOwned + ?Sized,
    T::Owned: FromScript,
{
    fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
        Ok(Cow::Owned(T::Owned::from_script(v)?))
    }
}

impl From<&str> for ScriptValue {
    fn from(s: &str) -> Self {
        Self::String(s.to_owned())
    }
}
