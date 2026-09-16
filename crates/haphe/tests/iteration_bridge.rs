//! Exercises the generated `meta_iter` / `meta_len` / `meta_index` /
//! `meta_newindex` registrations through a mock binder.

#![cfg(feature = "macros")]

use std::fmt;

use haphe::{Script, ScriptBind, ScriptConvertError, ScriptIter, ScriptValue, TypeBinder};

#[derive(Debug)]
struct NeverError;

impl fmt::Display for NeverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("never")
    }
}

impl std::error::Error for NeverError {}

type IndexFn<T> = fn(&T, &[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>;
type NewIndexFn<T> = fn(&mut T, &[ScriptValue]) -> Result<(), ScriptConvertError>;

#[derive(Default)]
struct MockBinder<T> {
    iter: Option<fn(T) -> ScriptIter>,
    iter_registrations: usize,
    len: Option<fn(T) -> usize>,
    index: Option<IndexFn<T>>,
    newindex: Option<NewIndexFn<T>>,
}

impl<T> TypeBinder<T> for MockBinder<T> {
    type Error = NeverError;

    fn field<V: haphe::IntoScript + haphe::FromScript + Clone + 'static>(
        &mut self,
        _: &'static str,
        _: fn(&T) -> V,
        _: Option<fn(&mut T, V)>,
    ) -> Result<(), NeverError> {
        Ok(())
    }

    fn method_ref(
        &mut self,
        _: &'static str,
        _: fn(&T, &[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>,
    ) -> Result<(), NeverError> {
        Ok(())
    }

    fn method_mut(
        &mut self,
        _: &'static str,
        _: fn(&mut T, &[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>,
    ) -> Result<(), NeverError> {
        Ok(())
    }

    fn method_owned(
        &mut self,
        _: &'static str,
        _: fn(T, &[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>,
    ) -> Result<(), NeverError> {
        Ok(())
    }

    fn constructor(
        &mut self,
        _: &'static str,
        _: fn(&[ScriptValue]) -> Result<T, ScriptConvertError>,
    ) -> Result<(), NeverError> {
        Ok(())
    }

    fn meta_tostring(&mut self, _: fn(&T) -> String) -> Result<(), NeverError> {
        Ok(())
    }

    fn meta_concat(&mut self, _: fn(&T) -> String) -> Result<(), NeverError> {
        Ok(())
    }

    fn meta_eq(&mut self, _: fn(&T, &T) -> bool) -> Result<(), NeverError> {
        Ok(())
    }

    fn meta_lt(&mut self, _: fn(&T, &T) -> bool) -> Result<(), NeverError> {
        Ok(())
    }

    fn meta_le(&mut self, _: fn(&T, &T) -> bool) -> Result<(), NeverError> {
        Ok(())
    }

    fn meta_unm(&mut self, _: fn(&T) -> T) -> Result<(), NeverError> {
        Ok(())
    }

    fn meta_arith_self(&mut self, _: &'static str, _: fn(T, T) -> T) -> Result<(), NeverError> {
        Ok(())
    }

    fn meta_arith_scalar(
        &mut self,
        _: &'static str,
        _: &'static haphe::TypeDescriptor<'static>,
        _: fn(T, &[ScriptValue]) -> Result<T, ScriptConvertError>,
    ) -> Result<(), NeverError> {
        Ok(())
    }

    fn meta_iter(&mut self, f: fn(T) -> ScriptIter) -> Result<(), NeverError> {
        self.iter = Some(f);
        self.iter_registrations += 1;
        Ok(())
    }

    fn meta_len(&mut self, f: fn(T) -> usize) -> Result<(), NeverError> {
        self.len = Some(f);
        Ok(())
    }

    fn meta_index(
        &mut self,
        f: fn(&T, &[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>,
    ) -> Result<(), NeverError> {
        self.index = Some(f);
        Ok(())
    }

    fn meta_newindex(
        &mut self,
        f: fn(&mut T, &[ScriptValue]) -> Result<(), ScriptConvertError>,
    ) -> Result<(), NeverError> {
        self.newindex = Some(f);
        Ok(())
    }
}

// A sequence-shaped iterable: items are plain values.
#[derive(Script, Clone)]
#[script(traits(IntoIterator(item = i64)))]
struct Sequence {
    #[script(skip)]
    values: Vec<i64>,
}

impl IntoIterator for Sequence {
    type Item = i64;
    type IntoIter = std::vec::IntoIter<i64>;
    fn into_iter(self) -> Self::IntoIter {
        self.values.into_iter()
    }
}

// A map-shaped iterable: items are 2-tuples, crossing as 2-element lists.
#[derive(Script)]
#[script(traits(IntoIterator(item = (String, i64))))]
struct Table {
    #[script(skip)]
    entries: Vec<(String, i64)>,
}

impl IntoIterator for Table {
    type Item = (String, i64);
    type IntoIter = std::vec::IntoIter<(String, i64)>;
    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

// An iterator type: bridges through the blanket `IntoIterator` impl.
#[derive(Script, Clone)]
#[script(traits(Iterator(item = i64)))]
struct Countdown {
    #[script(skip)]
    remaining: i64,
}

impl Iterator for Countdown {
    type Item = i64;
    fn next(&mut self) -> Option<i64> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        Some(self.remaining + 1)
    }
}

#[derive(Script)]
#[script(traits(Index(index = i64, output = i64), IndexMut(index = i64, output = i64)))]
struct Buffer {
    #[script(skip)]
    data: Vec<i64>,
}

impl std::ops::Index<i64> for Buffer {
    type Output = i64;
    fn index(&self, i: i64) -> &i64 {
        &self.data[usize::try_from(i).expect("negative index")]
    }
}

impl std::ops::IndexMut<i64> for Buffer {
    fn index_mut(&mut self, i: i64) -> &mut i64 {
        &mut self.data[usize::try_from(i).expect("negative index")]
    }
}

fn bound<T: ScriptBind>() -> MockBinder<T> {
    let mut binder = MockBinder {
        iter: None,
        iter_registrations: 0,
        len: None,
        index: None,
        newindex: None,
    };
    T::bind(&mut binder).unwrap();
    binder
}

#[test]
fn sequence_iterates_as_plain_values() {
    let binder = bound::<Sequence>();
    let iter = binder.iter.expect("meta_iter registered");
    let items: Vec<ScriptValue> = iter(Sequence {
        values: vec![10, 20, 30],
    })
    .collect();
    let ints: Vec<i64> = items
        .iter()
        .map(|v| match v {
            ScriptValue::I64(n) => *n,
            other => panic!("expected I64, got {other:?}"),
        })
        .collect();
    assert_eq!(ints, vec![10, 20, 30]);
}

#[test]
fn tuple_items_cross_as_two_element_lists() {
    let binder = bound::<Table>();
    let iter = binder.iter.expect("meta_iter registered");
    let items: Vec<ScriptValue> = iter(Table {
        entries: vec![("a".into(), 1)],
    })
    .collect();
    assert_eq!(items.len(), 1);
    match &items[0] {
        ScriptValue::List(pair) => {
            assert!(matches!(&pair[0], ScriptValue::String(s) if s == "a"));
            assert!(matches!(pair[1], ScriptValue::I64(1)));
        }
        other => panic!("expected List, got {other:?}"),
    }
}

#[test]
fn iterator_bridges_through_blanket_into_iterator() {
    let binder = bound::<Countdown>();
    let iter = binder.iter.expect("meta_iter registered");
    let items: Vec<ScriptValue> = iter(Countdown { remaining: 3 }).collect();
    assert_eq!(items.len(), 3);
    assert!(matches!(items[0], ScriptValue::I64(3)));
    assert!(matches!(items[2], ScriptValue::I64(1)));
}

#[test]
fn both_iterator_traits_register_once() {
    #[derive(Script, Clone)]
    #[script(traits(Iterator(item = i64), IntoIterator(item = i64)))]
    struct Twice {
        #[script(skip)]
        remaining: i64,
    }
    impl Iterator for Twice {
        type Item = i64;
        fn next(&mut self) -> Option<i64> {
            (self.remaining > 0).then(|| {
                self.remaining -= 1;
                self.remaining + 1
            })
        }
    }
    let binder = bound::<Twice>();
    assert_eq!(binder.iter_registrations, 1);
}

#[test]
fn len_comes_from_size_hint() {
    let binder = bound::<Sequence>();
    let len = binder.len.expect("meta_len registered");
    assert_eq!(len(Sequence { values: vec![1, 2] }), 2);
    assert_eq!(len(Sequence { values: vec![] }), 0);
}

#[test]
fn lazy_iterator_reports_size_hint() {
    let iter = ScriptIter::new((0..5).map(ScriptValue::I64));
    assert_eq!(iter.size_hint(), (5, Some(5)));
}

#[test]
fn index_reads_and_writes() {
    let binder = bound::<Buffer>();
    let index = binder.index.expect("meta_index registered");
    let newindex = binder.newindex.expect("meta_newindex registered");

    let mut buf = Buffer { data: vec![7, 8] };
    assert!(matches!(
        index(&buf, &[ScriptValue::I64(1)]).unwrap(),
        ScriptValue::I64(8)
    ));
    newindex(&mut buf, &[ScriptValue::I64(0), ScriptValue::I64(99)]).unwrap();
    assert_eq!(buf.data, vec![99, 8]);
}

#[test]
fn index_key_conversion_failure_errors() {
    let binder = bound::<Buffer>();
    let index = binder.index.expect("meta_index registered");
    let buf = Buffer { data: vec![1] };
    assert!(index(&buf, &[ScriptValue::String("x".into())]).is_err());
}

#[test]
fn non_iterable_registers_nothing() {
    #[derive(Script)]
    struct Plain {
        x: f64,
    }
    let binder = bound::<Plain>();
    assert!(binder.iter.is_none());
    assert!(binder.len.is_none());
    assert!(binder.index.is_none());
    assert!(binder.newindex.is_none());
}
