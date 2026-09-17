//! Container coverage beyond `Vec` and `HashMap<String, _>`: ordered maps,
//! sets, deques, fixed arrays, and slices. Sets and deques are described and
//! carried as lists; maps require string keys (the bridge's map carrier is
//! string-keyed).

use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};

use crate::bridge::{FromScript, ScriptConvertError, ScriptValue};
use crate::haphe_type::HapheType;
use crate::types::TypeDescriptor;

impl<V> From<BTreeMap<String, V>> for ScriptValue
where
    ScriptValue: From<V>,
{
    fn from(map: BTreeMap<String, V>) -> Self {
        Self::Map(
            map.into_iter()
                .map(|(k, v)| (k, ScriptValue::from(v)))
                .collect(),
        )
    }
}

impl<V: FromScript> FromScript for BTreeMap<String, V> {
    fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
        match v {
            ScriptValue::Map(entries) => entries
                .into_iter()
                .map(|(k, v)| Ok((k, V::from_script(v)?)))
                .collect(),
            other => Err(ScriptConvertError {
                expected: "map",
                got: other.variant_name(),
            }),
        }
    }
}

/// Described as a list; iteration order (and thus list order) is the set's
/// own — unspecified for `HashSet`.
impl<T: HapheType> HapheType for HashSet<T> {
    const DESCRIPTOR: TypeDescriptor<'static> = TypeDescriptor::List(&T::DESCRIPTOR);
}

impl<T> From<HashSet<T>> for ScriptValue
where
    ScriptValue: From<T>,
{
    fn from(set: HashSet<T>) -> Self {
        Self::List(set.into_iter().map(ScriptValue::from).collect())
    }
}

impl<T: FromScript + Eq + std::hash::Hash> FromScript for HashSet<T> {
    fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
        match v {
            ScriptValue::List(items) => items.into_iter().map(T::from_script).collect(),
            other => Err(ScriptConvertError {
                expected: "list",
                got: other.variant_name(),
            }),
        }
    }
}

/// Described as a list, carried in ascending order.
impl<T: HapheType> HapheType for BTreeSet<T> {
    const DESCRIPTOR: TypeDescriptor<'static> = TypeDescriptor::List(&T::DESCRIPTOR);
}

impl<T> From<BTreeSet<T>> for ScriptValue
where
    ScriptValue: From<T>,
{
    fn from(set: BTreeSet<T>) -> Self {
        Self::List(set.into_iter().map(ScriptValue::from).collect())
    }
}

impl<T: FromScript + Ord> FromScript for BTreeSet<T> {
    fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
        match v {
            ScriptValue::List(items) => items.into_iter().map(T::from_script).collect(),
            other => Err(ScriptConvertError {
                expected: "list",
                got: other.variant_name(),
            }),
        }
    }
}

/// Described as a list, front to back.
impl<T: HapheType> HapheType for VecDeque<T> {
    const DESCRIPTOR: TypeDescriptor<'static> = TypeDescriptor::List(&T::DESCRIPTOR);
}

impl<T> From<VecDeque<T>> for ScriptValue
where
    ScriptValue: From<T>,
{
    fn from(deque: VecDeque<T>) -> Self {
        Self::List(deque.into_iter().map(ScriptValue::from).collect())
    }
}

impl<T: FromScript> FromScript for VecDeque<T> {
    fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
        match v {
            ScriptValue::List(items) => items.into_iter().map(T::from_script).collect(),
            other => Err(ScriptConvertError {
                expected: "list",
                got: other.variant_name(),
            }),
        }
    }
}

impl<T, const N: usize> From<[T; N]> for ScriptValue
where
    ScriptValue: From<T>,
{
    fn from(array: [T; N]) -> Self {
        Self::List(array.into_iter().map(ScriptValue::from).collect())
    }
}

impl<T: FromScript, const N: usize> FromScript for [T; N] {
    fn from_script(v: ScriptValue) -> Result<Self, ScriptConvertError> {
        match v {
            ScriptValue::List(items) => {
                if items.len() != N {
                    return Err(ScriptConvertError {
                        expected: "array",
                        got: "list of mismatched length",
                    });
                }
                let converted: Vec<T> = items
                    .into_iter()
                    .map(T::from_script)
                    .collect::<Result<_, _>>()?;
                Ok(converted
                    .try_into()
                    .unwrap_or_else(|_| unreachable!("length checked")))
            }
            other => Err(ScriptConvertError {
                expected: "array",
                got: other.variant_name(),
            }),
        }
    }
}

impl<T: Clone> From<&[T]> for ScriptValue
where
    ScriptValue: From<T>,
{
    fn from(slice: &[T]) -> Self {
        Self::List(slice.iter().cloned().map(ScriptValue::from).collect())
    }
}
