//! Fallible surfaces: `Result<T, E>` constructors, methods, and free
//! functions bind, describing `T` and mapping `Err` into
//! [`ScriptCallError::Callee`] (rendered via `Display`, tagged with the
//! declared `error_kind`).

#![cfg(feature = "macros")]
#![allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    clippy::doc_markdown,
    reason = "fixture shapes are dictated by the bridge surface under test: `#[script]` functions receive OWNED values (the boundary contract), methods keep unused receivers, and docs name fixture idents verbatim"
)]

use std::fmt;

use haphe::{
    Script, ScriptBind, ScriptCallError, ScriptConvertError, ScriptIter, ScriptValue, TypeBinder,
    TypeDescriptor, script,
};

#[derive(Debug)]
struct NeverError;

impl fmt::Display for NeverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "never")
    }
}

impl std::error::Error for NeverError {}

type MethodFn<T> =
    for<'a> fn(haphe::ScriptCow<'a, T>, &[ScriptValue]) -> Result<ScriptValue, ScriptCallError>;
type MethodMutFn<T> = fn(&mut T, &[ScriptValue]) -> Result<ScriptValue, ScriptCallError>;
type CtorFn<T> = fn(&[ScriptValue]) -> Result<T, ScriptCallError>;
type AsyncFn<T> =
    for<'a> fn(haphe::ScriptCow<'a, T>, &'a [ScriptValue]) -> haphe::ScriptCallFuture<'a>;
type AsyncCtorFn<T> = for<'a> fn(&'a [ScriptValue]) -> haphe::ScriptCtorFuture<'a, T>;

#[derive(Default)]
struct Collect<T> {
    methods: Vec<(&'static str, MethodFn<T>)>,
    mut_methods: Vec<(&'static str, MethodMutFn<T>)>,
    ctors: Vec<(&'static str, CtorFn<T>)>,
    async_methods: Vec<(&'static str, AsyncFn<T>)>,
    async_ctors: Vec<(&'static str, AsyncCtorFn<T>)>,
}

impl<T> TypeBinder<T> for Collect<T> {
    type Error = NeverError;

    fn field<V: haphe::IntoScript + haphe::FromScript + Clone + 'static>(
        &mut self,
        _: &'static str,
        _: fn(&T) -> V,
        _: Option<fn(&mut T, V)>,
    ) -> Result<(), NeverError> {
        Ok(())
    }

    fn field_get<V: haphe::IntoScript + Clone + 'static>(
        &mut self,
        _: &'static str,
        _: fn(&T) -> V,
    ) -> Result<(), NeverError> {
        Ok(())
    }

    fn method(&mut self, name: &'static str, f: MethodFn<T>) -> Result<(), NeverError> {
        self.methods.push((name, f));
        Ok(())
    }

    fn method_mut(&mut self, name: &'static str, f: MethodMutFn<T>) -> Result<(), NeverError> {
        self.mut_methods.push((name, f));
        Ok(())
    }

    fn method_async(&mut self, name: &'static str, f: AsyncFn<T>) -> Result<(), NeverError> {
        self.async_methods.push((name, f));
        Ok(())
    }

    fn method_async_mut(
        &mut self,
        _: &'static str,
        _: for<'a> fn(&'a mut T, &'a [ScriptValue]) -> haphe::ScriptCallFuture<'a>,
    ) -> Result<(), NeverError> {
        Ok(())
    }

    fn constructor(&mut self, name: &'static str, f: CtorFn<T>) -> Result<(), NeverError> {
        self.ctors.push((name, f));
        Ok(())
    }

    fn associated(
        &mut self,
        _: &'static str,
        _: fn(&[ScriptValue]) -> Result<ScriptValue, ScriptCallError>,
    ) -> Result<(), NeverError> {
        Ok(())
    }

    fn associated_async(
        &mut self,
        _: &'static str,
        _: for<'a> fn(&'a [ScriptValue]) -> haphe::ScriptCallFuture<'a>,
    ) -> Result<(), NeverError> {
        Ok(())
    }

    fn constructor_async(
        &mut self,
        name: &'static str,
        f: AsyncCtorFn<T>,
    ) -> Result<(), NeverError> {
        self.async_ctors.push((name, f));
        Ok(())
    }

    fn property_get(
        &mut self,
        _: &'static str,
        _: fn(&T) -> ScriptValue,
    ) -> Result<(), NeverError> {
        Ok(())
    }

    fn property_set(
        &mut self,
        _: &'static str,
        _: fn(&mut T, ScriptValue) -> Result<(), ScriptConvertError>,
    ) -> Result<(), NeverError> {
        Ok(())
    }

    fn property_get_async(
        &mut self,
        _: &'static str,
        _: for<'a> fn(haphe::ScriptCow<'a, T>) -> haphe::ScriptCallFuture<'a>,
    ) -> Result<(), NeverError> {
        Ok(())
    }

    fn property_set_async(
        &mut self,
        _: &'static str,
        _: for<'a> fn(&'a mut T, ScriptValue) -> haphe::ScriptCallFuture<'a>,
    ) -> Result<(), NeverError> {
        Ok(())
    }

    fn meta_tostring(&mut self, _: fn(&T) -> String) -> Result<(), NeverError> {
        Ok(())
    }

    fn meta_concat(&mut self, _: fn(&T) -> String) -> Result<(), NeverError> {
        Ok(())
    }

    fn meta_hash(&mut self, _: fn(&T) -> u64) -> Result<(), NeverError> {
        Ok(())
    }

    fn meta_debug(&mut self, _: fn(&T) -> String) -> Result<(), NeverError> {
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

    fn meta_bnot(&mut self, _: fn(&T) -> T) -> Result<(), NeverError> {
        Ok(())
    }

    fn meta_arith_self(&mut self, _: &'static str, _: fn(T, T) -> T) -> Result<(), NeverError> {
        Ok(())
    }

    fn meta_arith_scalar(
        &mut self,
        _: &'static str,
        _: &'static TypeDescriptor<'static>,
        _: fn(T, &[ScriptValue]) -> Result<T, ScriptConvertError>,
    ) -> Result<(), NeverError> {
        Ok(())
    }

    fn meta_iter(&mut self, _: fn(T) -> ScriptIter) -> Result<(), NeverError> {
        Ok(())
    }

    fn meta_len(&mut self, _: fn(T) -> usize) -> Result<(), NeverError> {
        Ok(())
    }

    fn meta_call(
        &mut self,
        _: fn(&T, &[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>,
    ) -> Result<(), NeverError> {
        Ok(())
    }

    fn meta_call_async(&mut self, _: AsyncFn<T>) -> Result<(), NeverError> {
        Ok(())
    }

    fn meta_index(
        &mut self,
        _: fn(&T, &[ScriptValue]) -> Result<ScriptValue, ScriptConvertError>,
    ) -> Result<(), NeverError> {
        Ok(())
    }

    fn meta_newindex(
        &mut self,
        _: fn(&mut T, &[ScriptValue]) -> Result<(), ScriptConvertError>,
    ) -> Result<(), NeverError> {
        Ok(())
    }
}

fn bound<T: ScriptBind>() -> Collect<T> {
    let mut binder = Collect {
        methods: Vec::new(),
        mut_methods: Vec::new(),
        ctors: Vec::new(),
        async_methods: Vec::new(),
        async_ctors: Vec::new(),
    };
    T::bind(&mut binder).unwrap();
    binder
}

fn poll_ready<T>(fut: std::pin::Pin<Box<dyn std::future::Future<Output = T> + '_>>) -> T {
    use std::task::{Context, Poll, Waker};
    let mut fut = fut;
    let mut cx = Context::from_waker(Waker::noop());
    match fut.as_mut().poll(&mut cx) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!("future not ready"),
    }
}

#[derive(Script, Clone, Debug)]
#[script(thread_safety = send_sync, methods)]
struct Gauge {
    level: i64,
}

#[script]
impl Gauge {
    #[script(constructor, error_kind = "ValueError")]
    fn new(level: i64) -> Result<Self, TextError> {
        if level < 0 {
            Err(TextError(format!("negative level {level}")))
        } else {
            Ok(Gauge { level })
        }
    }

    #[script(constructor)]
    async fn fetch(level: i64) -> Result<Self, TextError> {
        if level < 0 {
            Err(TextError("negative".into()))
        } else {
            Ok(Gauge { level })
        }
    }

    #[script(error_kind = "RangeError")]
    fn checked_add(&self, amount: i64) -> Result<i64, TextError> {
        self.level
            .checked_add(amount)
            .ok_or_else(|| TextError("overflow".into()))
    }

    fn drain(&mut self, amount: i64) -> Result<(), TextError> {
        if amount > self.level {
            return Err(TextError("insufficient".into()));
        }
        self.level -= amount;
        Ok(())
    }

    async fn refresh(&self) -> Result<i64, TextError> {
        if self.level == 0 {
            Err(TextError("empty".into()))
        } else {
            Ok(self.level)
        }
    }
}

#[test]
fn descriptors_see_the_ok_type() {
    use haphe::ScriptImpl;
    let ctor = <Gauge as ScriptImpl>::CONSTRUCTORS
        .iter()
        .find(|c| c.name == "new")
        .unwrap();
    assert!(!matches!(*ctor.return_type, TypeDescriptor::Result(..)));
    assert_eq!(ctor.error_kind, Some("ValueError"));
    assert!(ctor.fallible);
    let add = <Gauge as ScriptImpl>::METHODS
        .iter()
        .find(|m| m.name == "checked_add")
        .unwrap();
    assert_eq!(
        *add.return_type,
        TypeDescriptor::Primitive(haphe::PrimitiveType::I64)
    );
}

#[test]
fn fallible_constructor_maps_err_to_host() {
    let binder = bound::<Gauge>();
    let (name, ctor) = binder.ctors[0];
    assert_eq!(name, "new");
    assert_eq!(ctor(&[ScriptValue::I64(3)]).unwrap().level, 3);
    match ctor(&[ScriptValue::I64(-1)]).unwrap_err() {
        ScriptCallError::Callee { error, kind, .. } => {
            assert_eq!(kind, Some("ValueError"));
            assert!(error.to_string().contains("negative level -1"));
        }
        ScriptCallError::Convert(other) => panic!("expected Callee error, got {other:?}"),
    }
}

#[test]
fn fallible_async_constructor_maps_err_to_host() {
    let binder = bound::<Gauge>();
    let (name, ctor) = binder.async_ctors[0];
    assert_eq!(name, "fetch");
    assert_eq!(poll_ready(ctor(&[ScriptValue::I64(5)])).unwrap().level, 5);
    match poll_ready(ctor(&[ScriptValue::I64(-1)])).unwrap_err() {
        ScriptCallError::Callee { kind: None, .. } => {}
        other => panic!("expected kind-less Host error, got {other:?}"),
    }
}

#[test]
fn fallible_methods_map_err_to_host() {
    let binder = bound::<Gauge>();
    let (_, f) = binder
        .methods
        .iter()
        .find(|(n, _)| *n == "checked_add")
        .unwrap();
    let gauge = Gauge { level: 40 };
    let out = f(haphe::ScriptCow::Borrowed(&gauge), &[ScriptValue::I64(2)]).unwrap();
    assert!(matches!(out, ScriptValue::I64(42)));
    match f(
        haphe::ScriptCow::Borrowed(&gauge),
        &[ScriptValue::I64(i64::MAX)],
    )
    .unwrap_err()
    {
        ScriptCallError::Callee { error, kind, .. } => {
            assert_eq!(kind, Some("RangeError"));
            assert_eq!(error.to_string(), "overflow");
        }
        ScriptCallError::Convert(other) => panic!("expected Callee error, got {other:?}"),
    }
}

#[test]
fn fallible_unit_methods_yield_unit_on_ok() {
    let binder = bound::<Gauge>();
    let (name, f) = binder.mut_methods[0];
    assert_eq!(name, "drain");
    let mut gauge = Gauge { level: 10 };
    let out = f(&mut gauge, &[ScriptValue::I64(4)]).unwrap();
    assert!(matches!(out, ScriptValue::Unit));
    assert_eq!(gauge.level, 6);
    assert!(matches!(
        f(&mut gauge, &[ScriptValue::I64(100)]).unwrap_err(),
        ScriptCallError::Callee { kind: None, .. }
    ));
}

#[test]
fn fallible_async_methods_map_err_to_host() {
    let binder = bound::<Gauge>();
    let (name, f) = binder.async_methods[0];
    assert_eq!(name, "refresh");
    let gauge = Gauge { level: 0 };
    let args: [ScriptValue; 0] = [];
    match poll_ready(f(haphe::ScriptCow::Borrowed(&gauge), &args)).unwrap_err() {
        ScriptCallError::Callee { error, .. } => assert_eq!(error.to_string(), "empty"),
        ScriptCallError::Convert(other) => panic!("expected Callee error, got {other:?}"),
    }
}

// Fallible free functions map identically.
#[script(error_kind = "ParseError")]
fn parse_level(raw: String) -> Result<i64, std::num::ParseIntError> {
    raw.parse()
}

#[test]
fn fallible_free_fns_map_err_to_host() {
    use haphe::{FnBinder, ScriptBindFn, ScriptFunction};
    type Wrapper = fn(&[ScriptValue]) -> Result<ScriptValue, ScriptCallError>;
    #[derive(Default)]
    struct CollectFn(Vec<(&'static str, Wrapper)>);
    impl FnBinder for CollectFn {
        type Error = std::convert::Infallible;
        fn function(
            &mut self,
            name: &'static str,
            _: &'static [TypeDescriptor<'static>],
            f: Wrapper,
        ) -> Result<(), Self::Error> {
            self.0.push((name, f));
            Ok(())
        }
        fn function_async(
            &mut self,
            _: &'static str,
            _: &'static [TypeDescriptor<'static>],
            _: for<'a> fn(&'a [ScriptValue]) -> haphe::ScriptCallFuture<'a>,
        ) -> Result<(), Self::Error> {
            Ok(())
        }
    }
    assert_eq!(
        *<parse_level as ScriptFunction>::DESCRIPTOR.return_type,
        TypeDescriptor::Primitive(haphe::PrimitiveType::I64)
    );
    let mut binder = CollectFn::default();
    <parse_level as ScriptBindFn>::bind(&mut binder).unwrap();
    let (_, f) = binder.0[0];
    let out = f(&[ScriptValue::String("42".into())]).unwrap();
    assert!(matches!(out, ScriptValue::I64(42)));
    match f(&[ScriptValue::String("nope".into())]).unwrap_err() {
        ScriptCallError::Callee { kind, .. } => assert_eq!(kind, Some("ParseError")),
        ScriptCallError::Convert(other) => panic!("expected Callee error, got {other:?}"),
    }
}

// A fallible free fn whose ok type isn't whitelist-recognized must not fall
// into the trait-presence dispatch path (which would emit ill-typed code);
// it stays descriptor-only. Compiling is the test.
#[derive(Script, Clone)]
struct Reading {
    value: f64,
}

#[script]
#[allow(
    dead_code,
    reason = "fixtures are exercised through their generated descriptors and bridge wrappers, not direct calls"
)]
fn parse_reading(raw: String) -> Result<Reading, TextError> {
    raw.parse()
        .map(|value| Reading { value })
        .map_err(|e| TextError(e.to_string()))
}

#[test]
fn fallible_non_whitelisted_ok_types_compile() {
    use haphe::ScriptFunction;
    assert!(matches!(
        *<parse_reading as ScriptFunction>::DESCRIPTOR.return_type,
        TypeDescriptor::Ref(_)
    ));
}

/// A message-only fixture error: `String` itself no longer crosses (host
/// errors must implement `std::error::Error`).
#[derive(Debug)]
struct TextError(String);

impl std::fmt::Display for TextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TextError {}

// ---------------------------------------------------------------------------
// The error object crosses intact: downcast, source chain, type name
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct RootCause;

impl std::fmt::Display for RootCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("root cause")
    }
}

impl std::error::Error for RootCause {}

#[derive(Debug)]
struct Layered {
    source: RootCause,
}

impl std::fmt::Display for Layered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("layered failure")
    }
}

impl std::error::Error for Layered {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

#[derive(Script, Clone)]
#[script(methods)]
struct Prober {
    n: i64,
}

#[script]
impl Prober {
    #[script(error_kind = "LayerError")]
    fn probe(&self) -> Result<i64, Layered> {
        Err(Layered { source: RootCause })
    }
}

#[test]
fn host_error_crosses_intact() {
    let binder = bound::<Prober>();
    let (_, f) = binder
        .methods
        .iter()
        .find(|(n, _)| *n == "probe")
        .expect("probe bound");
    let p = Prober { n: 0 };
    let err = f(haphe::ScriptCow::Borrowed(&p), &[]).unwrap_err();

    // Kind-prefixed Display rendering is unchanged.
    assert_eq!(err.to_string(), "LayerError: layered failure");
    assert_eq!(err.message(), "layered failure");
    // The source chain crosses (the error's own message excluded).
    assert_eq!(err.cause_chain(), vec!["root cause".to_string()]);

    let ScriptCallError::Callee {
        error,
        kind,
        type_name,
    } = err
    else {
        panic!("expected Callee");
    };
    assert_eq!(kind, Some("LayerError"));
    assert!(type_name.ends_with("Layered"), "{type_name}");
    // Host-side embedders recover the concrete type.
    let concrete = error.downcast_ref::<Layered>().expect("downcasts");
    assert_eq!(concrete.source.to_string(), "root cause");
    // And std::error::Error::source on the call error walks into it.
    let rebuilt = ScriptCallError::Callee {
        error: error.clone(),
        kind,
        type_name,
    };
    let source = std::error::Error::source(&rebuilt).expect("host source");
    assert_eq!(source.to_string(), "layered failure");
}
