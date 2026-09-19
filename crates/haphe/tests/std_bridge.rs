//! Free functions over commonly used std types bind through the widened
//! value-type gate: sets, ordered maps, paths, addresses, time tuples, and
//! `NonZero*` all compile wrappers and convert at the boundary.

#![cfg(feature = "macros")]
#![allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    clippy::doc_markdown,
    reason = "fixture shapes are dictated by the bridge surface under test: `#[script]` functions receive OWNED values (the boundary contract), methods keep unused receivers, and docs name fixture idents verbatim"
)]

use std::collections::{BTreeMap, HashSet};
use std::net::IpAddr;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::time::Duration;

use haphe::{FnBinder, ScriptBindFn, ScriptCallError, ScriptValue, TypeDescriptor, script};

#[script]
fn total(values: HashSet<i64>) -> i64 {
    values.into_iter().sum()
}

#[script]
fn keys(map: BTreeMap<String, i64>) -> Vec<String> {
    map.into_keys().collect()
}

#[script]
fn with_extension(path: PathBuf) -> PathBuf {
    path.with_extension("lua")
}

#[script]
fn is_loopback(addr: IpAddr) -> bool {
    addr.is_loopback()
}

#[script]
fn double(d: Duration) -> Duration {
    d * 2
}

#[script]
fn halve(n: NonZeroU32) -> u32 {
    n.get() / 2
}

#[script(dyn, instantiate(i64), instantiate(PathBuf))]
fn relay<T>(value: T) -> T {
    value
}

// Fully qualified std spellings pass the gates like their bare forms.
#[script]
fn qualified(
    path: std::path::PathBuf,
    pause: std::time::Duration,
    seen: std::collections::HashSet<i64>,
) -> std::path::PathBuf {
    let _ = (pause, seen);
    path
}

#[script]
fn unbox(value: Box<i64>) -> std::sync::Arc<String> {
    std::sync::Arc::new(value.to_string())
}

#[allow(
    clippy::elidable_lifetime_names,
    reason = "the NAMED lifetime is the surface under test: it must cross into the descriptor as `Borrowed { lifetime: Some(\"a\"), .. }` — eliding it would change what this fixture pins"
)]
#[script]
fn shout<'a>(text: std::borrow::Cow<'a, str>) -> std::borrow::Cow<'a, str> {
    std::borrow::Cow::Owned(text.to_uppercase())
}

#[script(dyn, instantiate(i64))]
fn share<T>(value: T) -> std::rc::Rc<T> {
    std::rc::Rc::new(value)
}

type Wrapper = fn(&[ScriptValue]) -> Result<ScriptValue, haphe::ScriptCallError>;

#[derive(Default)]
struct Collect(Vec<(&'static str, Wrapper)>);

impl FnBinder for Collect {
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

fn bind_one<F: ScriptBindFn>() -> (&'static str, Wrapper) {
    let mut binder = Collect::default();
    F::bind(&mut binder).unwrap();
    assert_eq!(binder.0.len(), 1);
    binder.0[0]
}

#[test]
fn set_params_bind_and_convert() {
    let (name, f) = bind_one::<total>();
    assert_eq!(name, "total");
    let out = f(&[ScriptValue::List(vec![
        ScriptValue::I64(2),
        ScriptValue::I64(3),
        ScriptValue::I64(2),
    ])])
    .unwrap();
    assert!(matches!(out, ScriptValue::I64(5)));
}

#[test]
fn btreemap_params_bind_and_convert() {
    let (_, f) = bind_one::<keys>();
    let out = f(&[ScriptValue::Map(vec![
        ("b".into(), ScriptValue::I64(2)),
        ("a".into(), ScriptValue::I64(1)),
    ])])
    .unwrap();
    match out {
        ScriptValue::List(items) => {
            assert!(matches!(&items[0], ScriptValue::String(s) if s == "a"));
        }
        other => panic!("expected list, got {other:?}"),
    }
}

#[test]
fn qualified_std_spellings_bind() {
    let (_, f) = bind_one::<qualified>();
    let out = f(&[
        ScriptValue::String("a/b".into()),
        ScriptValue::from((1u64, 0u32)),
        ScriptValue::List(vec![ScriptValue::I64(1)]),
    ])
    .unwrap();
    assert!(matches!(out, ScriptValue::String(s) if s == "a/b"));
}

#[test]
fn paths_bind_as_strings() {
    let (_, f) = bind_one::<with_extension>();
    let out = f(&[ScriptValue::String("notes/todo.txt".into())]).unwrap();
    assert!(matches!(out, ScriptValue::String(s) if s == "notes/todo.lua"));
}

#[test]
fn addresses_bind_as_strings() {
    let (_, f) = bind_one::<is_loopback>();
    let out = f(&[ScriptValue::String("127.0.0.1".into())]).unwrap();
    assert!(matches!(out, ScriptValue::Bool(true)));
    let err = f(&[ScriptValue::String("nope".into())]).unwrap_err();
    assert!(matches!(err, ScriptCallError::Convert(e) if e.got == "malformed address string"));
}

#[test]
fn durations_bind_as_tuples() {
    let (_, f) = bind_one::<double>();
    let out = f(&[ScriptValue::from((2u64, 500_000_000u32))]).unwrap();
    match out {
        ScriptValue::List(items) => {
            assert!(matches!(
                items[..],
                [ScriptValue::I64(5), ScriptValue::I64(0)]
            ));
        }
        other => panic!("expected tuple, got {other:?}"),
    }
}

#[test]
fn non_zero_binds_and_rejects_zero() {
    let (_, f) = bind_one::<halve>();
    let out = f(&[ScriptValue::I64(8)]).unwrap();
    assert!(matches!(out, ScriptValue::I64(4)));
    let err = f(&[ScriptValue::I64(0)]).unwrap_err();
    assert!(matches!(err, ScriptCallError::Convert(e) if e.got == "zero"));
}

#[test]
fn carriers_are_transparent_in_free_fns() {
    let (_, f) = bind_one::<unbox>();
    let out = f(&[ScriptValue::I64(6)]).unwrap();
    assert!(matches!(out, ScriptValue::String(s) if s == "6"));
    // A carrier around the generic parameter erases in dyn wrappers too.
    let mut binder = Collect::default();
    <share as ScriptBindFn>::bind(&mut binder).unwrap();
    let (_, f) = binder.0[0];
    let out = f(&[ScriptValue::I64(3)]).unwrap();
    assert!(matches!(out, ScriptValue::I64(3)));
}

// Carriers are transparent in struct fields and method signatures too:
// the registrations below compile only if the gates peel `Box`/`Rc`/`Arc`
// (conversion behavior is unit-tested in haphe-core).
#[derive(haphe::Script, Clone)]
#[script(methods)]
struct Carried {
    boxed: Box<i64>,
}

#[script]
impl Carried {
    fn label(&self, prefix: std::rc::Rc<String>) -> std::sync::Arc<String> {
        std::sync::Arc::new(format!("{prefix}{}", self.boxed))
    }

    fn suffixed(&self, base: std::borrow::Cow<'_, str>) -> String {
        format!("{base}-{}", self.boxed)
    }
}

#[test]
fn cow_is_transparent_in_free_fns() {
    let (_, f) = bind_one::<shout>();
    let out = f(&[ScriptValue::String("hey".into())]).unwrap();
    assert!(matches!(out, ScriptValue::String(s) if s == "HEY"));
}

#[test]
fn cow_descriptors_carry_the_declared_lifetime() {
    use haphe::ScriptFunction;
    let desc = <shout as ScriptFunction>::DESCRIPTOR;
    let expected = TypeDescriptor::Borrowed {
        lifetime: Some("a"),
        inner: &TypeDescriptor::String,
    };
    assert_eq!(*desc.params[0].ty, expected);
    assert_eq!(*desc.return_type, expected);
}

#[test]
fn std_semantic_types_are_valid_dyn_candidates() {
    use haphe::FunctionDescriptor;

    #[derive(Default)]
    struct CollectDyn(Vec<(&'static [TypeDescriptor<'static>], Wrapper)>);
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
            _: &'static FunctionDescriptor<'static>,
            type_args: &'static [TypeDescriptor<'static>],
            f: Wrapper,
        ) -> Result<(), Self::Error> {
            self.0.push((type_args, f));
            Ok(())
        }
    }

    let mut binder = CollectDyn::default();
    <relay as ScriptBindFn>::bind(&mut binder).unwrap();
    assert_eq!(binder.0.len(), 2);
    let (args, f) = binder.0[1];
    assert_eq!(args[0], TypeDescriptor::String);
    let out = f(&[ScriptValue::String("a/b".into())]).unwrap();
    assert!(matches!(out, ScriptValue::String(s) if s == "a/b"));
}
