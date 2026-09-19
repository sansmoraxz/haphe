//! `#[script]` on free functions exposes a descriptor through the
//! `ScriptFunction` trait, reachable through imports and re-exports.

#![cfg(feature = "macros")]

use haphe::{
    FunctionDescriptor, Ownership, ParamDescriptor, PrimitiveType, ScriptFunction, TypeDescriptor,
    script,
};

/// Adds two numbers.
#[script]
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[script(rename = "shout", error_kind = "ValueError")]
fn make_loud(text: &str) -> String {
    text.to_uppercase()
}

pub mod math {
    use haphe::script;

    #[script]
    pub fn mul(a: f64, b: f64) -> f64 {
        a * b
    }
}

// Descriptors travel with imports and renamed re-exports.
use math::mul;
use math::mul as multiply;

/// A function with explicit (but monomorphic) lifetimes is exposable.
#[allow(clippy::needless_lifetimes)]
#[script]
pub fn first_word<'a>(s: &'a str) -> &'a str {
    s.split_whitespace().next().unwrap_or("")
}

/// Raw identifiers are exposed without the `r#` prefix.
#[script]
pub fn r#loop(n: i32) -> i32 {
    n
}

const I32: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::I32);

static EXPECTED_ADD: FunctionDescriptor<'static> = FunctionDescriptor {
    name: "add",
    doc: Some("Adds two numbers."),
    receiver: None,
    generic_params: &[],
    instantiations: &[],
    dispatch: haphe::Dispatch::Static,
    params: &[
        ParamDescriptor {
            name: "a",
            ty: &I32,
            ownership: Ownership::Owned,
        },
        ParamDescriptor {
            name: "b",
            ty: &I32,
            ownership: Ownership::Owned,
        },
    ],
    return_type: &I32,
    return_ownership: Ownership::Owned,
    is_async: false,
    error_kind: None,
};

/// Descriptors are usable in static initializers (`registry!` relies on this).
static IN_STATIC: &[FunctionDescriptor<'static>] = &[
    <add as ScriptFunction>::DESCRIPTOR,
    <math::mul as ScriptFunction>::DESCRIPTOR,
];

#[test]
fn descriptor_matches_hand_written() {
    assert_eq!(<add as ScriptFunction>::DESCRIPTOR, EXPECTED_ADD);
    assert_eq!(add(2, 3), 5);
}

#[test]
fn rename_and_error_kind() {
    let desc = <make_loud as ScriptFunction>::DESCRIPTOR;
    assert_eq!(desc.name, "shout");
    assert_eq!(desc.error_kind, Some("ValueError"));
    assert_eq!(desc.params[0].ownership, Ownership::Ref);
    assert_eq!(*desc.params[0].ty, TypeDescriptor::String);
    assert_eq!(make_loud("hey"), "HEY");
}

#[test]
fn imports_and_reexports_resolve() {
    assert_eq!(IN_STATIC.len(), 2);
    assert_eq!(IN_STATIC[1].name, "mul");
    assert_eq!(<mul as ScriptFunction>::DESCRIPTOR.name, "mul");
    assert_eq!(<multiply as ScriptFunction>::DESCRIPTOR.name, "mul");
    assert_eq!(math::mul(2.0, 4.0), 8.0);
}

#[test]
fn lifetimes_are_erased() {
    let desc = <first_word as ScriptFunction>::DESCRIPTOR;
    assert_eq!(*desc.params[0].ty, TypeDescriptor::String);
    assert_eq!(*desc.return_type, TypeDescriptor::String);
    assert_eq!(desc.return_ownership, Ownership::Ref);
    assert_eq!(first_word("hello world"), "hello");
}

#[test]
fn raw_identifiers_are_unrawed() {
    assert_eq!(<r#loop as ScriptFunction>::DESCRIPTOR.name, "loop");
    assert_eq!(r#loop(7), 7);
}

// Async free functions register through `FnBinder::function_async` and the
// boxed future borrows the argument slice.
#[haphe::script]
async fn delayed_sum(a: i64, b: i64) -> i64 {
    a + b
}

#[test]
fn async_free_fn_binds_and_awaits() {
    use haphe::{FnBinder, ScriptBindFn, ScriptCallFuture, ScriptValue, TypeDescriptor};
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

    type AsyncWrapper = for<'a> fn(&'a [ScriptValue]) -> ScriptCallFuture<'a>;
    #[derive(Default)]
    struct CollectAsync(Vec<(&'static str, AsyncWrapper)>);
    impl FnBinder for CollectAsync {
        type Error = std::convert::Infallible;
        fn function(
            &mut self,
            _: &'static str,
            _: &'static [TypeDescriptor<'static>],
            _: fn(&[ScriptValue]) -> Result<ScriptValue, haphe::ScriptCallError>,
        ) -> Result<(), Self::Error> {
            Ok(())
        }
        fn function_async(
            &mut self,
            name: &'static str,
            _: &'static [TypeDescriptor<'static>],
            f: AsyncWrapper,
        ) -> Result<(), Self::Error> {
            self.0.push((name, f));
            Ok(())
        }
    }

    let mut binder = CollectAsync::default();
    <delayed_sum as ScriptBindFn>::bind(&mut binder).unwrap();
    let (name, f) = binder.0[0];
    assert_eq!(name, "delayed_sum");

    fn noop(_: *const ()) {}
    fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(std::ptr::null(), &VTABLE)
    }
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
    let mut cx = Context::from_waker(&waker);
    let args = [ScriptValue::I64(20), ScriptValue::I64(22)];
    let mut fut = f(&args);
    match fut.as_mut().poll(&mut cx) {
        Poll::Ready(Ok(ScriptValue::I64(42))) => {}
        other => panic!("expected Ready(I64(42)), got {other:?}"),
    }
}

// A STATIC async generic registers one monomorph per instantiation through
// `function_async`, keyed on (name, type_args) — same channel non-generic
// async functions use.
#[haphe::script(instantiate(i64), instantiate(String))]
async fn delayed_echo<T>(value: T) -> T {
    value
}

#[test]
fn async_static_generic_free_fn_binds_monomorphs() {
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    use haphe::{FnBinder, ScriptBindFn, ScriptCallFuture, ScriptValue, TypeDescriptor};

    type AsyncWrapper = for<'a> fn(&'a [ScriptValue]) -> ScriptCallFuture<'a>;
    #[derive(Default)]
    struct CollectAsync(
        Vec<(
            &'static str,
            &'static [TypeDescriptor<'static>],
            AsyncWrapper,
        )>,
    );
    impl FnBinder for CollectAsync {
        type Error = std::convert::Infallible;
        fn function(
            &mut self,
            _: &'static str,
            _: &'static [TypeDescriptor<'static>],
            _: fn(&[ScriptValue]) -> Result<ScriptValue, haphe::ScriptCallError>,
        ) -> Result<(), Self::Error> {
            panic!("async generic must not use the sync channel");
        }
        fn function_async(
            &mut self,
            name: &'static str,
            type_args: &'static [TypeDescriptor<'static>],
            f: AsyncWrapper,
        ) -> Result<(), Self::Error> {
            self.0.push((name, type_args, f));
            Ok(())
        }
    }

    let mut binder = CollectAsync::default();
    <delayed_echo as ScriptBindFn>::bind(&mut binder).unwrap();
    assert_eq!(binder.0.len(), 2);
    let (name, type_args, f) = binder.0[1];
    assert_eq!(name, "delayed_echo");
    assert_eq!(type_args[0], TypeDescriptor::String);
    let args = [ScriptValue::String("hi".into())];
    let fut = f(&args);
    let mut fut = pin!(fut);
    let mut cx = Context::from_waker(Waker::noop());
    match fut.as_mut().poll(&mut cx) {
        Poll::Ready(Ok(ScriptValue::String(s))) => assert_eq!(s, "hi"),
        other => panic!("unexpected poll result: {other:?}"),
    }
}

// A dyn generic registers one candidate per instantiation through the dyn
// channel, carrying the full descriptor for runtime ranking.
#[haphe::script(dyn, instantiate(i64), instantiate(String))]
fn relay_dyn<T>(value: T) -> T {
    value
}

#[test]
fn dyn_generic_registers_candidates_with_descriptor() {
    use haphe::{
        Dispatch, FnBinder, FunctionDescriptor, ScriptBindFn, ScriptFunction, TypeDescriptor,
    };

    use haphe::ScriptValue;
    type Wrapper = fn(&[ScriptValue]) -> Result<ScriptValue, haphe::ScriptCallError>;
    #[derive(Default)]
    struct CollectDyn(
        Vec<(
            &'static FunctionDescriptor<'static>,
            &'static [TypeDescriptor<'static>],
            Wrapper,
        )>,
    );
    impl FnBinder for CollectDyn {
        type Error = std::convert::Infallible;
        fn function(
            &mut self,
            _: &'static str,
            _: &'static [TypeDescriptor<'static>],
            _: Wrapper,
        ) -> Result<(), Self::Error> {
            panic!("dyn fn must not use the static channel");
        }
        fn function_async(
            &mut self,
            _: &'static str,
            _: &'static [TypeDescriptor<'static>],
            _: for<'a> fn(&'a [ScriptValue]) -> haphe::ScriptCallFuture<'a>,
        ) -> Result<(), Self::Error> {
            Ok(())
        }
        fn function_dyn(
            &mut self,
            descriptor: &'static FunctionDescriptor<'static>,
            type_args: &'static [TypeDescriptor<'static>],
            f: Wrapper,
        ) -> Result<(), Self::Error> {
            self.0.push((descriptor, type_args, f));
            Ok(())
        }
    }

    assert_eq!(
        <relay_dyn as ScriptFunction>::DESCRIPTOR.dispatch,
        Dispatch::Dyn
    );
    let mut binder = CollectDyn::default();
    <relay_dyn as ScriptBindFn>::bind(&mut binder).unwrap();
    assert_eq!(binder.0.len(), 2);
    let (desc, args, f) = binder.0[0];
    assert_eq!(desc.name, "relay_dyn");
    assert!(matches!(args[0], TypeDescriptor::Primitive(_)));
    let out = f(&[ScriptValue::I64(7)]).unwrap();
    assert!(matches!(out, ScriptValue::I64(7)));

    // The default trait impl delegates to `function` (verified by the
    // capability story; here the override captured instead).
}

// Bare `dyn` on a single-parameter generic auto-instantiates the default
// bridgeable candidate set, in the documented (tiebreak) order.
#[haphe::script(dyn)]
fn mirror<T: haphe::FromScript + haphe::IntoScript>(value: T) -> T {
    value
}

#[test]
fn bare_dyn_gets_default_candidate_set() {
    use haphe::{
        FnBinder, FunctionDescriptor, PrimitiveType, ScriptBindFn, ScriptValue, TypeDescriptor,
    };
    type Wrapper = fn(&[ScriptValue]) -> Result<ScriptValue, haphe::ScriptCallError>;
    #[derive(Default)]
    struct Collect(Vec<&'static [TypeDescriptor<'static>]>);
    impl FnBinder for Collect {
        type Error = std::convert::Infallible;
        fn function(
            &mut self,
            _: &'static str,
            _: &'static [TypeDescriptor<'static>],
            _: Wrapper,
        ) -> Result<(), Self::Error> {
            panic!("dyn must not use the static channel");
        }
        fn function_async(
            &mut self,
            _: &'static str,
            _: &'static [TypeDescriptor<'static>],
            _: for<'a> fn(&'a [ScriptValue]) -> haphe::ScriptCallFuture<'a>,
        ) -> Result<(), Self::Error> {
            Ok(())
        }
        fn function_dyn(
            &mut self,
            _: &'static FunctionDescriptor<'static>,
            type_args: &'static [TypeDescriptor<'static>],
            _: Wrapper,
        ) -> Result<(), Self::Error> {
            self.0.push(type_args);
            Ok(())
        }
    }
    let mut binder = Collect::default();
    <mirror as ScriptBindFn>::bind(&mut binder).unwrap();
    let kinds: Vec<&TypeDescriptor<'static>> = binder.0.iter().map(|a| &a[0]).collect();
    assert_eq!(
        kinds,
        vec![
            &TypeDescriptor::Primitive(PrimitiveType::I64),
            &TypeDescriptor::Primitive(PrimitiveType::F64),
            &TypeDescriptor::Primitive(PrimitiveType::Bool),
            &TypeDescriptor::String,
            &TypeDescriptor::Primitive(PrimitiveType::Char),
        ]
    );
}

// Container instantiations now compile wrappers (the freefn gate accepts
// standard containers of bridgeable values).
#[haphe::script(dyn, instantiate(Vec<i64>), instantiate(i64))]
fn total<T>(value: T) -> T {
    value
}

#[test]
fn container_instantiations_bind() {
    use haphe::{FnBinder, FunctionDescriptor, ScriptBindFn, ScriptValue, TypeDescriptor};
    type Wrapper = fn(&[ScriptValue]) -> Result<ScriptValue, haphe::ScriptCallError>;
    #[derive(Default)]
    struct Collect(Vec<Wrapper>);
    impl FnBinder for Collect {
        type Error = std::convert::Infallible;
        fn function(
            &mut self,
            _: &'static str,
            _: &'static [TypeDescriptor<'static>],
            _: Wrapper,
        ) -> Result<(), Self::Error> {
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
        fn function_dyn(
            &mut self,
            _: &'static FunctionDescriptor<'static>,
            _: &'static [TypeDescriptor<'static>],
            f: Wrapper,
        ) -> Result<(), Self::Error> {
            self.0.push(f);
            Ok(())
        }
    }
    let mut binder = Collect::default();
    <total as ScriptBindFn>::bind(&mut binder).unwrap();
    assert_eq!(binder.0.len(), 2, "container candidate compiled");
    let out = binder.0[0](&[ScriptValue::List(vec![ScriptValue::I64(1)])]).unwrap();
    assert!(matches!(out, ScriptValue::List(_)));
}
