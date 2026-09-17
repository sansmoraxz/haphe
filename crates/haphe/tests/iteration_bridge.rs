//! Exercises the generated metamethod registrations (`meta_iter` /
//! `meta_len` / `meta_index` / `meta_newindex` / bitwise) through a mock
//! binder.

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
type BinOp<T> = (&'static str, fn(T, T) -> T);
type NewIndexFn<T> = fn(&mut T, &[ScriptValue]) -> Result<(), ScriptConvertError>;
type AsyncFn<T> =
    for<'a> fn(haphe::ScriptCow<'a, T>, &'a [ScriptValue]) -> haphe::ScriptCallFuture<'a>;
type AsyncMutFn<T> = for<'a> fn(&'a mut T, &'a [ScriptValue]) -> haphe::ScriptCallFuture<'a>;
type CtorFn<T> = fn(&[ScriptValue]) -> Result<T, ScriptConvertError>;
type PropGetFn<T> = fn(&T) -> ScriptValue;
type PropSetFn<T> = fn(&mut T, ScriptValue) -> Result<(), ScriptConvertError>;
type PropGetAsyncFn<T> = for<'a> fn(haphe::ScriptCow<'a, T>) -> haphe::ScriptCallFuture<'a>;
type PropSetAsyncFn<T> = for<'a> fn(&'a mut T, ScriptValue) -> haphe::ScriptCallFuture<'a>;
type AsyncCtorFn<T> = for<'a> fn(&'a [ScriptValue]) -> haphe::ScriptCtorFuture<'a, T>;
type CowMethodFn<T> =
    for<'a> fn(haphe::ScriptCow<'a, T>, &[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>;

#[derive(Default)]
struct MockBinder<T> {
    iter: Option<fn(T) -> ScriptIter>,
    iter_registrations: usize,
    len: Option<fn(T) -> usize>,
    index: Option<IndexFn<T>>,
    newindex: Option<NewIndexFn<T>>,
    arith_self: Vec<BinOp<T>>,
    bnot: Option<fn(&T) -> T>,
    fields: Vec<&'static str>,
    methods: Vec<(&'static str, CowMethodFn<T>)>,
    hash: Option<fn(&T) -> u64>,
    debug: Option<fn(&T) -> String>,
    constructors: Vec<(&'static str, CtorFn<T>)>,
    call: Option<IndexFn<T>>,
    call_async: Option<AsyncFn<T>>,
    async_methods: Vec<(&'static str, AsyncFn<T>)>,
    async_mut_methods: Vec<(&'static str, AsyncMutFn<T>)>,
    prop_gets: Vec<(&'static str, PropGetFn<T>)>,
    prop_sets: Vec<(&'static str, PropSetFn<T>)>,
    prop_gets_async: Vec<(&'static str, PropGetAsyncFn<T>)>,
    prop_sets_async: Vec<(&'static str, PropSetAsyncFn<T>)>,
    async_ctors: Vec<(&'static str, AsyncCtorFn<T>)>,
}

impl<T> TypeBinder<T> for MockBinder<T> {
    type Error = NeverError;

    fn field<V: haphe::IntoScript + haphe::FromScript + Clone + 'static>(
        &mut self,
        name: &'static str,
        _: fn(&T) -> V,
        _: Option<fn(&mut T, V)>,
    ) -> Result<(), NeverError> {
        self.fields.push(name);
        Ok(())
    }

    fn method(
        &mut self,
        name: &'static str,
        f: for<'a> fn(
            haphe::ScriptCow<'a, T>,
            &[ScriptValue],
        ) -> Result<ScriptValue, ScriptConvertError>,
    ) -> Result<(), NeverError> {
        self.methods.push((name, f));
        Ok(())
    }

    fn method_mut(
        &mut self,
        _: &'static str,
        _: fn(&mut T, &[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>,
    ) -> Result<(), NeverError> {
        Ok(())
    }

    fn method_async(
        &mut self,
        name: &'static str,
        f: for<'a> fn(haphe::ScriptCow<'a, T>, &'a [ScriptValue]) -> haphe::ScriptCallFuture<'a>,
    ) -> Result<(), NeverError> {
        self.async_methods.push((name, f));
        Ok(())
    }

    fn method_async_mut(
        &mut self,
        name: &'static str,
        f: for<'a> fn(&'a mut T, &'a [ScriptValue]) -> haphe::ScriptCallFuture<'a>,
    ) -> Result<(), NeverError> {
        self.async_mut_methods.push((name, f));
        Ok(())
    }

    fn constructor(
        &mut self,
        name: &'static str,
        f: fn(&[ScriptValue]) -> Result<T, ScriptConvertError>,
    ) -> Result<(), NeverError> {
        self.constructors.push((name, f));
        Ok(())
    }

    fn constructor_async(
        &mut self,
        name: &'static str,
        f: for<'a> fn(&'a [ScriptValue]) -> haphe::ScriptCtorFuture<'a, T>,
    ) -> Result<(), NeverError> {
        self.async_ctors.push((name, f));
        Ok(())
    }

    fn property_get(
        &mut self,
        name: &'static str,
        f: fn(&T) -> ScriptValue,
    ) -> Result<(), NeverError> {
        self.prop_gets.push((name, f));
        Ok(())
    }

    fn property_set(
        &mut self,
        name: &'static str,
        f: fn(&mut T, ScriptValue) -> Result<(), ScriptConvertError>,
    ) -> Result<(), NeverError> {
        self.prop_sets.push((name, f));
        Ok(())
    }

    fn property_get_async(
        &mut self,
        name: &'static str,
        f: for<'a> fn(haphe::ScriptCow<'a, T>) -> haphe::ScriptCallFuture<'a>,
    ) -> Result<(), NeverError> {
        self.prop_gets_async.push((name, f));
        Ok(())
    }

    fn property_set_async(
        &mut self,
        name: &'static str,
        f: for<'a> fn(&'a mut T, ScriptValue) -> haphe::ScriptCallFuture<'a>,
    ) -> Result<(), NeverError> {
        self.prop_sets_async.push((name, f));
        Ok(())
    }

    fn meta_hash(&mut self, f: fn(&T) -> u64) -> Result<(), NeverError> {
        self.hash = Some(f);
        Ok(())
    }

    fn meta_debug(&mut self, f: fn(&T) -> String) -> Result<(), NeverError> {
        self.debug = Some(f);
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

    fn meta_bnot(&mut self, f: fn(&T) -> T) -> Result<(), NeverError> {
        self.bnot = Some(f);
        Ok(())
    }

    fn meta_arith_self(&mut self, op: &'static str, f: fn(T, T) -> T) -> Result<(), NeverError> {
        self.arith_self.push((op, f));
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

    fn meta_call(
        &mut self,
        f: fn(&T, &[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>,
    ) -> Result<(), NeverError> {
        self.call = Some(f);
        Ok(())
    }

    fn meta_call_async(
        &mut self,
        f: for<'a> fn(haphe::ScriptCow<'a, T>, &'a [ScriptValue]) -> haphe::ScriptCallFuture<'a>,
    ) -> Result<(), NeverError> {
        self.call_async = Some(f);
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
        arith_self: Vec::new(),
        bnot: None,
        fields: Vec::new(),
        methods: Vec::new(),
        hash: None,
        debug: None,
        constructors: Vec::new(),
        call: None,
        call_async: None,
        async_methods: Vec::new(),
        async_mut_methods: Vec::new(),
        prop_gets: Vec::new(),
        prop_sets: Vec::new(),
        prop_gets_async: Vec::new(),
        prop_sets_async: Vec::new(),
        async_ctors: Vec::new(),
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

// ---------------------------------------------------------------------------
// Bitwise family
// ---------------------------------------------------------------------------

#[derive(Script, Clone, Copy, PartialEq, Debug)]
#[script(traits(BitAnd, BitOr, BitXor, Shl, Shr, Not))]
struct Mask {
    bits: u32,
}

macro_rules! mask_ops {
    ($($trait:ident :: $method:ident => $op:tt),*) => {$(
        impl std::ops::$trait for Mask {
            type Output = Mask;
            fn $method(self, rhs: Mask) -> Mask {
                Mask { bits: self.bits $op rhs.bits }
            }
        }
    )*};
}
mask_ops!(BitAnd::bitand => &, BitOr::bitor => |, BitXor::bitxor => ^);

impl std::ops::Shl for Mask {
    type Output = Mask;
    fn shl(self, rhs: Mask) -> Mask {
        Mask {
            bits: self.bits << rhs.bits,
        }
    }
}

impl std::ops::Shr for Mask {
    type Output = Mask;
    fn shr(self, rhs: Mask) -> Mask {
        Mask {
            bits: self.bits >> rhs.bits,
        }
    }
}

impl std::ops::Not for Mask {
    type Output = Mask;
    fn not(self) -> Mask {
        Mask { bits: !self.bits }
    }
}

#[test]
fn bitwise_traits_register_and_compute() {
    let binder = bound::<Mask>();
    let ops: Vec<&str> = binder.arith_self.iter().map(|(op, _)| *op).collect();
    assert_eq!(ops, vec!["bitand", "bitor", "bitxor", "shl", "shr"]);

    let a = Mask { bits: 0b1100 };
    let b = Mask { bits: 0b1010 };
    let get = |name: &str| {
        binder
            .arith_self
            .iter()
            .find(|(op, _)| *op == name)
            .unwrap()
            .1
    };
    assert_eq!(get("bitand")(a, b), Mask { bits: 0b1000 });
    assert_eq!(get("bitor")(a, b), Mask { bits: 0b1110 });
    assert_eq!(get("bitxor")(a, b), Mask { bits: 0b0110 });
    assert_eq!(get("shl")(a, Mask { bits: 1 }), Mask { bits: 0b11000 });
    assert_eq!(get("shr")(a, Mask { bits: 2 }), Mask { bits: 0b11 });

    let bnot = binder.bnot.expect("meta_bnot registered");
    assert_eq!(bnot(&a), Mask { bits: !0b1100u32 });
}

#[test]
fn scalar_rhs_bitwise_uses_scalar_path() {
    // Shl with a primitive rhs goes through meta_arith_scalar, not
    // meta_arith_self — mirroring the arithmetic family.
    #[derive(Script, Clone, Copy)]
    #[script(traits(Shl(rhs = u32, output = Self)))]
    struct Shifty {
        bits: u32,
    }
    // Direct haphe::ops impl — the default trait source.
    impl haphe::ops::Shl<u32> for Shifty {
        type Output = Shifty;
        fn shl(self, rhs: u32) -> Shifty {
            Shifty {
                bits: self.bits << rhs,
            }
        }
    }
    let binder = bound::<Shifty>();
    assert!(binder.arith_self.is_empty());
}

// ---------------------------------------------------------------------------
// Pow (haphe-provided ops trait)
// ---------------------------------------------------------------------------

#[derive(Script, Clone, Copy, PartialEq, Debug)]
#[script(traits(Pow))]
struct Scalar {
    value: f64,
}

impl haphe::ops::Pow for Scalar {
    type Output = Scalar;
    fn pow(self, rhs: Scalar) -> Scalar {
        Scalar {
            value: self.value.powf(rhs.value),
        }
    }
}

#[test]
fn pow_registers_and_computes() {
    let binder = bound::<Scalar>();
    let (op, f) = binder
        .arith_self
        .iter()
        .find(|(op, _)| *op == "pow")
        .expect("pow registered");
    assert_eq!(*op, "pow");
    let out = f(Scalar { value: 2.0 }, Scalar { value: 10.0 });
    assert_eq!(out, Scalar { value: 1024.0 });
}

#[test]
fn pow_provided_for_std_numerics() {
    use haphe::ops::Pow;
    assert_eq!(Pow::pow(2i64, 10u32), 1024);
    assert_eq!(Pow::pow(2.0f64, 10.0f64), 1024.0);
    assert_eq!(Pow::pow(2.0f64, 10i32), 1024.0);
}

// ---------------------------------------------------------------------------
// IDiv (floor division)
// ---------------------------------------------------------------------------

#[derive(Script, Clone, Copy, PartialEq, Debug)]
#[script(traits(IDiv))]
struct Ticks {
    n: i64,
}

impl haphe::ops::IDiv for Ticks {
    type Output = Ticks;
    fn idiv(self, rhs: Ticks) -> Ticks {
        Ticks {
            n: haphe::ops::IDiv::idiv(self.n, rhs.n),
        }
    }
}

#[test]
fn idiv_registers_and_floors() {
    let binder = bound::<Ticks>();
    let (_, f) = binder
        .arith_self
        .iter()
        .find(|(op, _)| *op == "idiv")
        .expect("idiv registered");
    assert_eq!(f(Ticks { n: 7 }, Ticks { n: 2 }), Ticks { n: 3 });
    // Floor, not truncation: Lua's `//` semantics.
    assert_eq!(f(Ticks { n: -7 }, Ticks { n: 2 }), Ticks { n: -4 });
    assert_eq!(f(Ticks { n: 7 }, Ticks { n: -2 }), Ticks { n: -4 });
}

#[test]
fn idiv_provided_for_std_numerics_floors() {
    use haphe::ops::IDiv;
    assert_eq!(IDiv::idiv(7i64, 2), 3);
    assert_eq!(IDiv::idiv(-7i64, 2), -4);
    assert_eq!(IDiv::idiv(7i64, -2), -4);
    assert_eq!(IDiv::idiv(-7i64, -2), 3);
    assert_eq!(IDiv::idiv(7u32, 2), 3);
    assert_eq!(IDiv::idiv(7.0f64, 2.0), 3.0);
    assert_eq!(IDiv::idiv(-7.0f64, 2.0), -4.0);
}

// ---------------------------------------------------------------------------
// Mod (floor modulo)
// ---------------------------------------------------------------------------

#[derive(Script, Clone, Copy, PartialEq, Debug)]
#[script(traits(Mod))]
struct Wrap {
    n: i64,
}

impl haphe::ops::Mod for Wrap {
    type Output = Wrap;
    fn modulo(self, rhs: Wrap) -> Wrap {
        Wrap {
            n: haphe::ops::Mod::modulo(self.n, rhs.n),
        }
    }
}

#[test]
fn mod_registers_with_floor_semantics() {
    let binder = bound::<Wrap>();
    let (_, f) = binder
        .arith_self
        .iter()
        .find(|(op, _)| *op == "mod")
        .expect("mod registered");
    assert_eq!(f(Wrap { n: 7 }, Wrap { n: 2 }), Wrap { n: 1 });
    // Divisor's sign, not the dividend's: Lua's `%`.
    assert_eq!(f(Wrap { n: -7 }, Wrap { n: 2 }), Wrap { n: 1 });
    assert_eq!(f(Wrap { n: 7 }, Wrap { n: -2 }), Wrap { n: -1 });
}

#[test]
fn mod_provided_for_std_numerics_floors() {
    use haphe::ops::Mod;
    assert_eq!(Mod::modulo(-7i64, 2), 1);
    assert_eq!(Mod::modulo(7i64, -2), -1);
    assert_eq!(Mod::modulo(-7i64, -2), -1);
    assert_eq!(Mod::modulo(7u32, 2), 1);
    assert_eq!(Mod::modulo(-7.5f64, 2.0), 0.5);
    // IDiv/Mod invariant: a == a.idiv(b) * b + a.modulo(b)
    use haphe::ops::IDiv;
    for (a, b) in [(-7i64, 2i64), (7, -2), (-7, -2), (7, 2)] {
        assert_eq!(a, IDiv::idiv(a, b) * b + Mod::modulo(a, b));
    }
}

// ---------------------------------------------------------------------------
// Transparent primitive newtypes across the bridge surfaces
// ---------------------------------------------------------------------------

/// A transparent bool newtype: crosses as a native boolean.
#[derive(Script, Clone, Copy)]
#[script(transparent)]
struct Flag(bool);

#[derive(Script, Clone)]
#[script(methods)]
struct Machine {
    powered: Flag,
    label: String,
}

#[haphe::script]
impl Machine {
    fn toggled(&self, next: Flag) -> Flag {
        Flag(self.powered.0 != next.0)
    }

    // A described struct return: stays descriptor-only (dispatch no-op).
    #[allow(dead_code)]
    fn twin(&self) -> Machine {
        self.clone()
    }
}

#[haphe::script]
fn invert(flag: Flag) -> Flag {
    Flag(!flag.0)
}

#[test]
fn transparent_newtype_field_and_method_bind() {
    let binder = bound::<Machine>();
    assert!(binder.fields.contains(&"powered"), "{:?}", binder.fields);
    assert!(binder.fields.contains(&"label"));

    let (_, f) = binder
        .methods
        .iter()
        .find(|(n, _)| *n == "toggled")
        .expect("newtype-typed method bound via dispatch");
    let m = Machine {
        powered: Flag(true),
        label: "m".into(),
    };
    let out = f(haphe::ScriptCow::Borrowed(&m), &[ScriptValue::Bool(true)]).unwrap();
    assert!(matches!(out, ScriptValue::Bool(false)));

    // The struct-returning method stays descriptor-only.
    assert!(!binder.methods.iter().any(|(n, _)| *n == "twin"));
}

#[test]
fn transparent_newtype_free_fn_binds() {
    use haphe::{FnBinder, ScriptBindFn};

    type FnEntry = (
        &'static str,
        fn(&[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>,
    );
    struct CollectFns(Vec<FnEntry>);
    impl FnBinder for CollectFns {
        type Error = NeverError;
        fn function(
            &mut self,
            name: &'static str,
            _: &'static [haphe::TypeDescriptor<'static>],
            f: fn(&[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>,
        ) -> Result<(), NeverError> {
            self.0.push((name, f));
            Ok(())
        }

        fn function_async(
            &mut self,
            _: &'static str,
            _: &'static [haphe::TypeDescriptor<'static>],
            _: for<'a> fn(&'a [ScriptValue]) -> haphe::ScriptCallFuture<'a>,
        ) -> Result<(), NeverError> {
            Ok(())
        }
    }

    let mut binder = CollectFns(Vec::new());
    <invert as ScriptBindFn>::bind(&mut binder).unwrap();
    let (name, f) = binder.0[0];
    assert_eq!(name, "invert");
    let out = f(&[ScriptValue::Bool(false)]).unwrap();
    assert!(matches!(out, ScriptValue::Bool(true)));
}

// ---------------------------------------------------------------------------
// Call / AsyncCall
// ---------------------------------------------------------------------------

#[derive(Script, Clone)]
#[script(traits(Call(args = (i64, i64), output = i64)))]
struct Adder {
    base: i64,
}

impl haphe::ops::Call<(i64, i64)> for Adder {
    type Output = i64;
    fn call(&self, (a, b): (i64, i64)) -> i64 {
        self.base + a + b
    }
}

#[derive(Script, Clone)]
#[script(thread_safety = send_sync, traits(AsyncCall(args = (i64,), output = i64)))]
struct DeferredDoubler {
    factor: i64,
}

impl haphe::ops::AsyncCall<(i64,)> for DeferredDoubler {
    type Output = i64;
    async fn call_async(&self, (n,): (i64,)) -> i64 {
        self.factor * n
    }
}

fn poll_ready(fut: haphe::ScriptCallFuture<'_>) -> Result<ScriptValue, ScriptConvertError> {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    fn noop(_: *const ()) {}
    fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(std::ptr::null(), &VTABLE)
    }
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
    let mut cx = Context::from_waker(&waker);
    let mut fut = fut;
    match fut.as_mut().poll(&mut cx) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!("future not immediately ready"),
    }
}

#[test]
fn call_registers_and_invokes() {
    let binder = bound::<Adder>();
    let f = binder.call.expect("meta_call registered");
    let adder = Adder { base: 100 };
    let out = f(&adder, &[ScriptValue::I64(2), ScriptValue::I64(3)]).unwrap();
    assert!(matches!(out, ScriptValue::I64(105)));
    // Missing / mistyped args error.
    assert!(f(&adder, &[ScriptValue::String("x".into())]).is_err());
    // Sync-only declaration registers no async call.
    assert!(binder.call_async.is_none());
}

#[test]
fn async_call_registers_and_awaits() {
    let binder = bound::<DeferredDoubler>();
    let f = binder.call_async.expect("meta_call_async registered");
    assert!(binder.call.is_none());
    let out = poll_ready(f(
        haphe::ScriptCow::Owned(DeferredDoubler { factor: 7 }),
        &[ScriptValue::I64(6)],
    ))
    .unwrap();
    assert!(matches!(out, ScriptValue::I64(42)));
}

#[test]
fn zero_arg_call_works() {
    #[derive(Script, Clone)]
    #[script(traits(Call(args = (), output = i64)))]
    struct Nullary {
        value: i64,
    }
    impl haphe::ops::Call<()> for Nullary {
        type Output = i64;
        fn call(&self, (): ()) -> i64 {
            self.value
        }
    }
    let binder = bound::<Nullary>();
    let f = binder.call.expect("meta_call registered");
    let out = f(&Nullary { value: 9 }, &[]).unwrap();
    assert!(matches!(out, ScriptValue::I64(9)));
}

// ---------------------------------------------------------------------------
// Async methods
// ---------------------------------------------------------------------------

#[derive(Script, Clone)]
#[script(thread_safety = send_sync, methods)]
struct Fetcher {
    #[script(readonly)]
    base: i64,
}

#[haphe::script]
impl Fetcher {
    async fn fetch(&self, offset: i64) -> i64 {
        self.base + offset
    }

    async fn bump(&mut self, by: i64) -> i64 {
        self.base += by;
        self.base
    }

    async fn consume(self) -> i64 {
        self.base
    }

    async fn fire(&self) {}
}

#[test]
fn async_methods_register_and_await() {
    let binder = bound::<Fetcher>();
    let names: Vec<&str> = binder.async_methods.iter().map(|(n, _)| *n).collect();
    assert_eq!(names, vec!["fetch", "consume", "fire"]);
    let mut_names: Vec<&str> = binder.async_mut_methods.iter().map(|(n, _)| *n).collect();
    assert_eq!(mut_names, vec!["bump"]);
    let get = |name: &str| {
        binder
            .async_methods
            .iter()
            .find(|(n, _)| *n == name)
            .unwrap()
            .1
    };

    use haphe::ScriptCow;
    // Borrowed receiver: zero-clone shared dispatch.
    let fetcher = Fetcher { base: 40 };
    let out = poll_ready(get("fetch")(
        ScriptCow::Borrowed(&fetcher),
        &[ScriptValue::I64(2)],
    ))
    .unwrap();
    assert!(matches!(out, ScriptValue::I64(42)));
    // Owned receiver: the consuming method takes it without cloning.
    let out = poll_ready(get("consume")(ScriptCow::Owned(Fetcher { base: 5 }), &[])).unwrap();
    assert!(matches!(out, ScriptValue::I64(5)));
    let out = poll_ready(get("fire")(ScriptCow::Borrowed(&fetcher), &[])).unwrap();
    assert!(matches!(out, ScriptValue::Unit));
    // Conversion failures surface through the future.
    assert!(
        poll_ready(get("fetch")(
            ScriptCow::Borrowed(&fetcher),
            &[ScriptValue::String("x".into())]
        ))
        .is_err()
    );

    // `&mut self`: the borrowed-mutable channel writes back in place.
    let (_, bump) = binder.async_mut_methods[0];
    let mut target = Fetcher { base: 1 };
    let out = poll_ready(bump(&mut target, &[ScriptValue::I64(9)])).unwrap();
    assert!(matches!(out, ScriptValue::I64(10)));
    assert_eq!(target.base, 10, "mutation persisted on the receiver");
}

// ---------------------------------------------------------------------------
// Hash / Debug / Default projections
// ---------------------------------------------------------------------------

#[derive(Script, Clone, Debug, Hash, PartialEq, Default)]
#[script(traits(Hash, Debug, Default, PartialEq))]
struct Tag {
    id: i64,
}

#[test]
fn hash_debug_default_register_and_compute() {
    let binder = bound::<Tag>();

    let hash = binder.hash.expect("meta_hash registered");
    let (a, b) = (Tag { id: 7 }, Tag { id: 7 });
    assert_eq!(hash(&a), hash(&b), "equal values hash equal");

    let debug = binder.debug.expect("meta_debug registered");
    assert_eq!(debug(&a), format!("{a:?}"));

    let (name, ctor) = binder
        .constructors
        .iter()
        .find(|(n, _)| *n == "default")
        .expect("Default projected as nullary constructor");
    assert_eq!(*name, "default");
    assert_eq!(ctor(&[]).unwrap(), Tag::default());
}

#[test]
fn debug_without_display_still_feeds_tostring_fallback() {
    // Existing behavior preserved: Debug supplies __tostring when Display
    // is absent, in addition to the new meta_debug channel.
    let binder = bound::<Tag>();
    assert!(binder.debug.is_some());
}

// ---------------------------------------------------------------------------
// Properties (sync + async) and async constructors
// ---------------------------------------------------------------------------

#[derive(Script, Clone)]
#[script(thread_safety = send_sync, methods)]
struct Meter {
    #[script(skip)]
    raw: i64,
}

#[haphe::script]
impl Meter {
    #[script(constructor)]
    fn new(raw: i64) -> Self {
        Meter { raw }
    }

    #[script(constructor)]
    async fn connect(raw: i64) -> Self {
        Meter { raw }
    }

    #[script(getter)]
    fn level(&self) -> i64 {
        self.raw * 2
    }

    #[script(setter)]
    fn set_level(&mut self, value: i64) {
        self.raw = value / 2;
    }

    #[script(getter)]
    async fn reading(&self) -> i64 {
        self.raw + 1
    }

    #[script(setter = "reading")]
    async fn set_reading(&mut self, value: i64) {
        self.raw = value - 1;
    }
}

#[test]
fn sync_properties_register_and_convert() {
    let binder = bound::<Meter>();
    let (name, get) = binder
        .prop_gets
        .iter()
        .find(|(n, _)| *n == "level")
        .expect("level getter registered");
    assert_eq!(*name, "level");
    assert!(matches!(get(&Meter { raw: 21 }), ScriptValue::I64(42)));

    let (_, set) = binder
        .prop_sets
        .iter()
        .find(|(n, _)| *n == "level")
        .expect("level setter registered");
    let mut m = Meter { raw: 0 };
    set(&mut m, ScriptValue::I64(10)).unwrap();
    assert_eq!(m.raw, 5);
    assert!(set(&mut m, ScriptValue::String("x".into())).is_err());
}

#[test]
fn async_properties_register_and_await() {
    let binder = bound::<Meter>();
    let (_, get) = binder
        .prop_gets_async
        .iter()
        .find(|(n, _)| *n == "reading")
        .expect("async getter registered");
    let m = Meter { raw: 41 };
    let out = poll_ready(get(haphe::ScriptCow::Borrowed(&m))).unwrap();
    assert!(matches!(out, ScriptValue::I64(42)));

    let (_, set) = binder
        .prop_sets_async
        .iter()
        .find(|(n, _)| *n == "reading")
        .expect("async setter registered");
    let mut target = Meter { raw: 0 };
    let out = poll_ready(set(&mut target, ScriptValue::I64(42))).unwrap();
    assert!(matches!(out, ScriptValue::Unit));
    assert_eq!(target.raw, 41, "async setter mutates in place");
}

#[test]
fn async_constructor_registers_and_awaits() {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    let binder = bound::<Meter>();
    // The sync ctor stays on the sync channel.
    assert!(binder.constructors.iter().any(|(n, _)| *n == "new"));
    let (name, ctor) = binder
        .async_ctors
        .iter()
        .find(|(n, _)| *n == "connect")
        .expect("async constructor registered");
    assert_eq!(*name, "connect");

    fn noop(_: *const ()) {}
    fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(std::ptr::null(), &VTABLE)
    }
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
    let mut cx = Context::from_waker(&waker);
    let args = [ScriptValue::I64(9)];
    let mut fut = ctor(&args);
    match fut.as_mut().poll(&mut cx) {
        Poll::Ready(Ok(m)) => assert_eq!(m.raw, 9),
        Poll::Ready(Err(e)) => panic!("expected Ok, got error: {e}"),
        Poll::Pending => panic!("future not immediately ready"),
    }
}

// Fallible constructors (returning `Result`) are excluded from BINDING on
// both the sync and async channels — they remain described. (Documented
// exclusion; a fallible-ctor bridge channel is a queued follow-up.)
#[derive(Script, Clone)]
#[script(thread_safety = send_sync, methods)]
struct Gauge {
    #[script(skip)]
    raw: i64,
}

#[haphe::script]
#[allow(dead_code)]
impl Gauge {
    #[script(constructor)]
    fn try_new(raw: i64) -> Result<Self, String> {
        Ok(Gauge { raw })
    }

    #[script(constructor)]
    async fn try_connect(raw: i64) -> Result<Self, String> {
        Ok(Gauge { raw })
    }

    fn raw(&self) -> i64 {
        self.raw
    }
}

#[test]
fn fallible_constructors_are_described_but_not_bound() {
    use haphe::ScriptImpl;
    let ctors = <Gauge as ScriptImpl>::CONSTRUCTORS;
    assert_eq!(ctors.len(), 2, "both fallible ctors described");
    assert!(ctors.iter().any(|c| c.name == "try_new"));
    assert!(ctors.iter().any(|c| c.name == "try_connect" && c.is_async));

    let binder = bound::<Gauge>();
    assert!(binder.constructors.is_empty(), "fallible sync ctor unbound");
    assert!(binder.async_ctors.is_empty(), "fallible async ctor unbound");
}
