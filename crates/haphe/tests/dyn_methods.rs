//! `dyn`-dispatched generic methods register one monomorph wrapper per
//! declared instantiation through the descriptor-carrying `method_dyn*`
//! channels, on structs and enums alike.

#![cfg(feature = "macros")]

use std::fmt;

use haphe::{
    Dispatch, FunctionDescriptor, Script, ScriptBind, ScriptCallError, ScriptConvertError,
    ScriptIter, ScriptValue, TypeBinder, TypeDescriptor, script,
};

#[derive(Debug)]
struct NeverError;

impl fmt::Display for NeverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "never")
    }
}

impl std::error::Error for NeverError {}

type Desc = &'static FunctionDescriptor<'static>;
type Args = &'static [TypeDescriptor<'static>];
type DynFn<T> =
    for<'a> fn(haphe::ScriptCow<'a, T>, &[ScriptValue]) -> Result<ScriptValue, ScriptCallError>;
type DynMutFn<T> = fn(&mut T, &[ScriptValue]) -> Result<ScriptValue, ScriptCallError>;
type DynAsyncFn<T> =
    for<'a> fn(haphe::ScriptCow<'a, T>, &'a [ScriptValue]) -> haphe::ScriptCallFuture<'a>;

#[derive(Default)]
struct DynBinder<T> {
    dyn_methods: Vec<(Desc, Args, DynFn<T>)>,
    dyn_mut_methods: Vec<(Desc, Args, DynMutFn<T>)>,
    dyn_async_methods: Vec<(Desc, Args, DynAsyncFn<T>)>,
    dyn_async_mut_methods: Vec<(Desc, Args, DynAsyncMutFn<T>)>,
    generic_methods: Vec<(&'static str, Args, DynFn<T>)>,
    generic_mut_methods: Vec<(&'static str, Args, DynMutFn<T>)>,
    generic_async_methods: Vec<(&'static str, Args, DynAsyncFn<T>)>,
    self_insts: Vec<haphe::SelfInstantiation>,
    plain_methods: Vec<&'static str>,
}

type DynAsyncMutFn<T> = for<'a> fn(&'a mut T, &'a [ScriptValue]) -> haphe::ScriptCallFuture<'a>;

impl<T> TypeBinder<T> for DynBinder<T> {
    type Error = NeverError;

    fn field<V: haphe::IntoScript + haphe::FromScript + Clone + 'static>(
        &mut self,
        _: &'static str,
        _: fn(&T) -> V,
        _: Option<fn(&mut T, V)>,
    ) -> Result<(), NeverError> {
        Ok(())
    }

    fn method(&mut self, name: &'static str, _: DynFn<T>) -> Result<(), NeverError> {
        self.plain_methods.push(name);
        Ok(())
    }

    fn method_mut(&mut self, _: &'static str, _: DynMutFn<T>) -> Result<(), NeverError> {
        Ok(())
    }

    fn method_async(&mut self, _: &'static str, _: DynAsyncFn<T>) -> Result<(), NeverError> {
        Ok(())
    }

    fn method_async_mut(
        &mut self,
        _: &'static str,
        _: for<'a> fn(&'a mut T, &'a [ScriptValue]) -> haphe::ScriptCallFuture<'a>,
    ) -> Result<(), NeverError> {
        Ok(())
    }

    fn method_generic(
        &mut self,
        name: &'static str,
        args: Args,
        f: DynFn<T>,
    ) -> Result<(), NeverError> {
        self.generic_methods.push((name, args, f));
        Ok(())
    }

    fn method_generic_mut(
        &mut self,
        name: &'static str,
        args: Args,
        f: DynMutFn<T>,
    ) -> Result<(), NeverError> {
        self.generic_mut_methods.push((name, args, f));
        Ok(())
    }

    fn method_generic_async(
        &mut self,
        name: &'static str,
        args: Args,
        f: DynAsyncFn<T>,
    ) -> Result<(), NeverError> {
        self.generic_async_methods.push((name, args, f));
        Ok(())
    }

    fn method_dyn(
        &mut self,
        d: Desc,
        args: Args,
        self_inst: haphe::SelfInstantiation,
        f: DynFn<T>,
    ) -> Result<(), NeverError> {
        self.dyn_methods.push((d, args, f));
        self.self_insts.push(self_inst);
        Ok(())
    }

    fn method_dyn_mut(
        &mut self,
        d: Desc,
        args: Args,
        _: haphe::SelfInstantiation,
        f: DynMutFn<T>,
    ) -> Result<(), NeverError> {
        self.dyn_mut_methods.push((d, args, f));
        Ok(())
    }

    fn method_dyn_async(
        &mut self,
        d: Desc,
        args: Args,
        _: haphe::SelfInstantiation,
        f: DynAsyncFn<T>,
    ) -> Result<(), NeverError> {
        self.dyn_async_methods.push((d, args, f));
        Ok(())
    }

    fn method_dyn_async_mut(
        &mut self,
        d: Desc,
        args: Args,
        _: haphe::SelfInstantiation,
        f: DynAsyncMutFn<T>,
    ) -> Result<(), NeverError> {
        self.dyn_async_mut_methods.push((d, args, f));
        Ok(())
    }

    fn constructor(
        &mut self,
        _: &'static str,
        _: fn(&[ScriptValue]) -> Result<T, ScriptCallError>,
    ) -> Result<(), NeverError> {
        Ok(())
    }

    fn constructor_async(
        &mut self,
        _: &'static str,
        _: for<'a> fn(&'a [ScriptValue]) -> haphe::ScriptCtorFuture<'a, T>,
    ) -> Result<(), NeverError> {
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

    fn meta_call_async(&mut self, _: DynAsyncFn<T>) -> Result<(), NeverError> {
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

fn bound<T: ScriptBind>() -> DynBinder<T> {
    let mut binder = DynBinder {
        dyn_methods: Vec::new(),
        dyn_mut_methods: Vec::new(),
        dyn_async_methods: Vec::new(),
        dyn_async_mut_methods: Vec::new(),
        generic_methods: Vec::new(),
        generic_mut_methods: Vec::new(),
        generic_async_methods: Vec::new(),
        self_insts: Vec::new(),
        plain_methods: Vec::new(),
    };
    T::bind(&mut binder).unwrap();
    binder
}

#[derive(Script, Clone)]
#[script(thread_safety = send_sync, methods)]
struct Holder {
    total: i64,
}

trait Acc {
    fn acc(self) -> i64;
}

impl Acc for i64 {
    fn acc(self) -> i64 {
        self
    }
}

impl Acc for f64 {
    fn acc(self) -> i64 {
        self as i64
    }
}

#[script]
impl Holder {
    /// Bare `dyn` on a single-parameter generic: default candidate set.
    #[script(dyn)]
    fn mirror<T>(&self, value: T) -> T {
        value
    }

    /// Explicit instantiations on a `&mut self` dyn method.
    #[script(dyn, instantiate(i64), instantiate(f64))]
    fn accumulate<T: Acc>(&mut self, value: T) {
        self.total += value.acc();
    }

    /// Async dyn method.
    #[script(dyn, instantiate(String))]
    async fn tag<T: ToString>(&self, value: T) -> String {
        value.to_string()
    }

    /// Async `&mut self` dyn method: the future borrows the receiver, so
    /// mutation writes back in place.
    #[script(dyn, instantiate(i64))]
    async fn bump<T: Acc>(&mut self, value: T) {
        self.total += value.acc();
    }

    /// Static dispatch: declared monomorphs keyed on (name, type_args), no
    /// runtime scan.
    #[script(instantiate(i64), instantiate(String))]
    fn first_of<T>(&self, a: T, b: T) -> T {
        let _ = b;
        a
    }

    /// Static dispatch on a `&mut self` generic.
    #[script(instantiate(f64))]
    fn add_in<T: Acc>(&mut self, value: T) {
        self.total += value.acc();
    }

    /// Async static generic: monomorphs through the async sibling channel.
    #[script(instantiate(i64))]
    async fn tag_static<T: ToString>(&self, value: T) -> String {
        value.to_string()
    }

    /// Non-generic methods still take the plain channel.
    fn get(&self) -> i64 {
        self.total
    }
}

#[test]
fn bare_dyn_method_gets_default_candidate_set() {
    let binder = bound::<Holder>();
    let mirrors: Vec<_> = binder
        .dyn_methods
        .iter()
        .filter(|(d, _, _)| d.name == "mirror")
        .collect();
    assert_eq!(mirrors.len(), 5);
    let expect = [
        TypeDescriptor::Primitive(haphe::PrimitiveType::I64),
        TypeDescriptor::Primitive(haphe::PrimitiveType::F64),
        TypeDescriptor::Primitive(haphe::PrimitiveType::Bool),
        TypeDescriptor::String,
        TypeDescriptor::Primitive(haphe::PrimitiveType::Char),
    ];
    for ((_, args, _), want) in mirrors.iter().zip(&expect) {
        assert_eq!(args.len(), 1);
        assert_eq!(&args[0], want);
    }
    let desc = mirrors[0].0;
    assert_eq!(desc.dispatch, Dispatch::Dyn);
    assert_eq!(desc.generic_params.len(), 1);
    assert_eq!(desc.instantiations.len(), 5);
    // Every candidate shares the one descriptor.
    assert!(mirrors.iter().all(|(d, _, _)| std::ptr::eq(*d, desc)));
}

#[test]
fn dyn_candidate_wrappers_are_monomorphized() {
    let binder = bound::<Holder>();
    let holder = Holder { total: 0 };
    let (_, _, int_wrapper) = binder
        .dyn_methods
        .iter()
        .find(|(d, args, _)| {
            d.name == "mirror" && args[0] == TypeDescriptor::Primitive(haphe::PrimitiveType::I64)
        })
        .unwrap();
    let out = int_wrapper(haphe::ScriptCow::Borrowed(&holder), &[ScriptValue::I64(41)]).unwrap();
    assert!(matches!(out, ScriptValue::I64(41)));
    let (_, _, str_wrapper) = binder
        .dyn_methods
        .iter()
        .find(|(d, args, _)| d.name == "mirror" && args[0] == TypeDescriptor::String)
        .unwrap();
    let out = str_wrapper(
        haphe::ScriptCow::Borrowed(&holder),
        &[ScriptValue::String("hi".into())],
    )
    .unwrap();
    assert!(matches!(out, ScriptValue::String(s) if s == "hi"));
    // The i64 wrapper's FromScript stays authoritative: a string arg errors,
    // which is the resolver's try-call fall-through signal.
    assert!(
        int_wrapper(
            haphe::ScriptCow::Borrowed(&holder),
            &[ScriptValue::String("nope".into())],
        )
        .is_err()
    );
}

#[test]
fn mut_dyn_methods_use_the_mut_channel_and_write_back() {
    let binder = bound::<Holder>();
    assert_eq!(binder.dyn_mut_methods.len(), 2);
    let mut holder = Holder { total: 0 };
    let (desc, args, wrapper) = &binder.dyn_mut_methods[0];
    assert_eq!(desc.name, "accumulate");
    assert_eq!(
        args[0],
        TypeDescriptor::Primitive(haphe::PrimitiveType::I64)
    );
    wrapper(&mut holder, &[ScriptValue::I64(5)]).unwrap();
    assert_eq!(holder.total, 5);
}

#[test]
fn async_dyn_methods_use_the_async_channel() {
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    let binder = bound::<Holder>();
    assert_eq!(binder.dyn_async_methods.len(), 1);
    let (desc, args, wrapper) = &binder.dyn_async_methods[0];
    assert_eq!(desc.name, "tag");
    assert!(desc.is_async);
    assert_eq!(args[0], TypeDescriptor::String);
    let holder = Holder { total: 0 };
    let script_args = [ScriptValue::String("x".into())];
    let fut = wrapper(haphe::ScriptCow::Borrowed(&holder), &script_args);
    let mut fut = pin!(fut);
    let mut cx = Context::from_waker(Waker::noop());
    match fut.as_mut().poll(&mut cx) {
        Poll::Ready(Ok(ScriptValue::String(s))) => assert_eq!(s, "x"),
        other => panic!("unexpected poll result: {other:?}"),
    }
}

#[test]
fn async_mut_dyn_methods_write_back_in_place() {
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    let binder = bound::<Holder>();
    assert_eq!(binder.dyn_async_mut_methods.len(), 1);
    let (desc, args, wrapper) = &binder.dyn_async_mut_methods[0];
    assert_eq!(desc.name, "bump");
    assert_eq!(
        args[0],
        TypeDescriptor::Primitive(haphe::PrimitiveType::I64)
    );
    let mut holder = Holder { total: 1 };
    let script_args = [ScriptValue::I64(9)];
    {
        let fut = wrapper(&mut holder, &script_args);
        let mut fut = pin!(fut);
        let mut cx = Context::from_waker(Waker::noop());
        assert!(matches!(
            fut.as_mut().poll(&mut cx),
            Poll::Ready(Ok(ScriptValue::Unit))
        ));
    }
    assert_eq!(holder.total, 10);
}

#[test]
fn static_generic_methods_register_monomorphs() {
    use haphe::ScriptImpl;
    let binder = bound::<Holder>();
    let firsts: Vec<_> = binder
        .generic_methods
        .iter()
        .filter(|(name, _, _)| *name == "first_of")
        .collect();
    assert_eq!(firsts.len(), 2);
    let (_, args, wrapper) = firsts[1];
    assert_eq!(args[0], TypeDescriptor::String);
    let holder = Holder { total: 0 };
    let out = wrapper(
        haphe::ScriptCow::Borrowed(&holder),
        &[
            ScriptValue::String("a".into()),
            ScriptValue::String("b".into()),
        ],
    )
    .unwrap();
    assert!(matches!(out, ScriptValue::String(s) if s == "a"));
    // The descriptor stays Static-dispatch.
    assert!(Holder::METHODS.iter().any(|m| m.name == "first_of"
        && m.dispatch == Dispatch::Static
        && m.instantiations.len() == 2));

    assert_eq!(binder.generic_mut_methods.len(), 1);
    let (name, args, wrapper) = &binder.generic_mut_methods[0];
    assert_eq!(*name, "add_in");
    assert_eq!(
        args[0],
        TypeDescriptor::Primitive(haphe::PrimitiveType::F64)
    );
    let mut holder = Holder { total: 1 };
    wrapper(&mut holder, &[ScriptValue::F64(4.0)]).unwrap();
    assert_eq!(holder.total, 5);
}

#[test]
fn async_static_generic_methods_use_the_async_sibling_channel() {
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    let binder = bound::<Holder>();
    assert_eq!(binder.generic_async_methods.len(), 1);
    let (name, args, wrapper) = &binder.generic_async_methods[0];
    assert_eq!(*name, "tag_static");
    assert_eq!(
        args[0],
        TypeDescriptor::Primitive(haphe::PrimitiveType::I64)
    );
    let holder = Holder { total: 0 };
    let script_args = [ScriptValue::I64(9)];
    let fut = wrapper(haphe::ScriptCow::Borrowed(&holder), &script_args);
    let mut fut = pin!(fut);
    let mut cx = Context::from_waker(Waker::noop());
    match fut.as_mut().poll(&mut cx) {
        Poll::Ready(Ok(ScriptValue::String(s))) => assert_eq!(s, "9"),
        other => panic!("unexpected poll result: {other:?}"),
    }
}

#[test]
fn plain_methods_keep_the_plain_channel() {
    let binder = bound::<Holder>();
    assert_eq!(binder.plain_methods, vec!["get"]);
}

#[derive(Script, Clone)]
#[script(methods)]
enum Mode {
    Fast,
    Slow,
}

#[script]
impl Mode {
    #[script(dyn, instantiate(i64), instantiate(bool))]
    fn pick<T>(&self, a: T, b: T) -> T {
        match self {
            Mode::Fast => a,
            Mode::Slow => b,
        }
    }
}

#[test]
fn enum_methods_take_the_dyn_channel_too() {
    let binder = bound::<Mode>();
    assert_eq!(binder.dyn_methods.len(), 2);
    let (desc, args, wrapper) = &binder.dyn_methods[1];
    assert_eq!(desc.name, "pick");
    assert_eq!(
        args[0],
        TypeDescriptor::Primitive(haphe::PrimitiveType::Bool)
    );
    let out = wrapper(
        haphe::ScriptCow::Borrowed(&Mode::Slow),
        &[ScriptValue::Bool(true), ScriptValue::Bool(false)],
    )
    .unwrap();
    assert!(matches!(out, ScriptValue::Bool(false)));
}

// ── Dyn methods on GENERIC self types: candidates carry the self
// instantiation so the resolver can substitute the type's parameters
// alongside the method's own.

#[derive(Script, Clone)]
#[script(methods)]
struct Pair<T: Clone + 'static> {
    left: T,
    right: T,
}

#[script]
impl<T: Clone> Pair<T> {
    #[script(dyn, instantiate(i64), instantiate(String))]
    fn tag_with<U>(&self, marker: U) -> U {
        let _ = &self.left;
        marker
    }

    /// A parameter referencing the SELF type's parameter: resolvable only
    /// through the carried self instantiation.
    #[script(dyn, instantiate(bool))]
    fn replace_left<U>(&mut self, value: T, flag: U) -> U {
        self.left = value;
        flag
    }

    /// STATIC monomorphs also work on generic self types: the registration
    /// surface is per-monomorph, so (name, type_args) stays unambiguous.
    #[script(instantiate(f64))]
    fn sized<U>(&self, scale: U) -> U {
        let _ = &self.right;
        scale
    }
}

#[test]
fn generic_self_dyn_methods_carry_the_self_instantiation() {
    let binder = bound::<Pair<i64>>();
    // tag_with: two candidates; replace_left: one (mut channel).
    assert_eq!(binder.dyn_methods.len(), 2);
    assert_eq!(binder.dyn_mut_methods.len(), 1);
    for inst in &binder.self_insts {
        assert_eq!(inst.params.len(), 1);
        assert_eq!(inst.params[0].name, "T");
        assert_eq!(
            inst.args[0],
            TypeDescriptor::Primitive(haphe::PrimitiveType::I64)
        );
    }
    // Wrappers are monomorphized over BOTH parameter kinds.
    let (desc, args, wrapper) = &binder.dyn_methods[1];
    assert_eq!(desc.name, "tag_with");
    assert_eq!(args[0], TypeDescriptor::String);
    let pair = Pair {
        left: 1i64,
        right: 2,
    };
    let out = wrapper(
        haphe::ScriptCow::Borrowed(&pair),
        &[ScriptValue::String("x".into())],
    )
    .unwrap();
    assert!(matches!(out, ScriptValue::String(s) if s == "x"));
    // The T-typed parameter converts through the monomorph's concrete type.
    let (desc, _, wrapper) = &binder.dyn_mut_methods[0];
    assert_eq!(desc.name, "replace_left");
    let mut pair = Pair {
        left: 1i64,
        right: 2,
    };
    let out = wrapper(&mut pair, &[ScriptValue::I64(9), ScriptValue::Bool(true)]).unwrap();
    assert!(matches!(out, ScriptValue::Bool(true)));
    assert_eq!(pair.left, 9);
    // The descriptor references BOTH generic params for the resolver.
    assert!(
        desc.params
            .iter()
            .any(|p| matches!(*p.ty, TypeDescriptor::GenericParam("T")))
    );
}

#[test]
fn static_generic_methods_work_on_generic_self_types() {
    let binder = bound::<Pair<i64>>();
    // sized<f64> is a static monomorph on Pair<i64>: (name, type_args) is
    // unambiguous because the registration surface is per-monomorph.
    let (name, args, wrapper) = &binder.generic_methods[0];
    assert_eq!(*name, "sized");
    assert_eq!(
        args[0],
        TypeDescriptor::Primitive(haphe::PrimitiveType::F64)
    );
    let pair = Pair {
        left: 3i64,
        right: 4,
    };
    let out = wrapper(haphe::ScriptCow::Borrowed(&pair), &[ScriptValue::F64(0.5)]).unwrap();
    assert!(matches!(out, ScriptValue::F64(v) if v == 0.5));
}
