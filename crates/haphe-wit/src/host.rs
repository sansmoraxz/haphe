//! Host-side dispatch tables for live provided-direction binding.
//!
//! [`WasmBinder`](crate::WasmBinder) registration (`register_type` and
//! friends) collects each type's bridge fn pointers through a
//! [`TypeBinder`] pass into type-erased tables; the linker closures built in
//! `runtime.rs` dispatch through them. Live resource values are owned by a
//! [`HostTable`] shared between all closures — the 32-bit canonical-ABI
//! `rep` of every handle is a key into it.

use std::any::Any;
use std::collections::HashMap;
use std::convert::Infallible;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};

use haphe::{
    FromScript, IntoScript, OpaqueUserData, ScriptCallError, ScriptCallFuture, ScriptConvertError,
    ScriptCow, ScriptCtorFuture, ScriptIter, ScriptValue, TypeBinder, TypeDescriptor,
};
use wasmtime::component::ResourceAny;

type Sv = ScriptValue;
type Sce = ScriptConvertError;
/// Wrapper-channel error: conversion failures plus Host errors from
/// fallible Rust implementations.
type Ce = ScriptCallError;

/// Boxed live host value stored behind a resource handle.
pub(crate) type AnyBox = Box<dyn Any + Send>;

/// Marker host type used in `ResourceType::host` for every haphe resource.
/// One marker for all types: per-entry [`HostEntry::type_name`] provides the
/// dynamic check that traps descriptively on cross-type handle misuse.
pub(crate) struct HostRep;

/// Live host values backing resource handles, keyed by handle `rep`.
pub(crate) struct HostTable {
    entries: Mutex<HashMap<u32, HostEntry>>,
    next: AtomicU32,
}

pub(crate) struct HostEntry {
    pub type_name: &'static str,
    pub value: AnyBox,
}

impl HostTable {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            next: AtomicU32::new(1),
        }
    }

    pub fn insert(&self, type_name: &'static str, value: AnyBox) -> u32 {
        let rep = self.next.fetch_add(1, Ordering::Relaxed);
        self.entries
            .lock()
            .expect("host table poisoned")
            .insert(rep, HostEntry { type_name, value });
        rep
    }

    pub fn remove(&self, rep: u32) -> Option<HostEntry> {
        self.entries
            .lock()
            .expect("host table poisoned")
            .remove(&rep)
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u32, HostEntry>> {
        self.entries.lock().expect("host table poisoned")
    }
}

// ---------------------------------------------------------------------------
// UserData payloads for resource handles crossing as values
// ---------------------------------------------------------------------------

/// A guest-owned resource handle held by the host, produced by lifting a
/// foreign call's resource return. Lowering it as an `own` parameter *takes*
/// the handle (reuse errors descriptively); dropping the wrapper queues the
/// handle on the owning caller's deferred-drop queue (drained on the next
/// dispatch, or via the caller's explicit flush).
/// `ScriptValue::UserData` payload for a HOST-owned resource: a rep in the
/// binder's [`HostTable`]. Lowering into a guest call lazily mints ONE
/// `own` handle over the rep (wasmtime borrows it in-call for borrow
/// positions, so the payload stays reusable); an `own` position TRANSFERS
/// the handle to the guest, whose eventual drop runs the registered
/// destructor — reuse after transfer errors descriptively. If the payload
/// is dropped un-transferred after minting, the handle (and table entry)
/// lives until store teardown. Mint with `WasmBinder::host_resource`.
pub struct HostResource {
    pub(crate) state: std::sync::Mutex<HostResState>,
}

pub(crate) enum HostResState {
    /// Table rep not yet minted into a store handle.
    Unminted(u32),
    /// A live `own` handle, reusable for borrow positions.
    Minted(wasmtime::component::ResourceAny),
    /// Ownership handed to the guest.
    Transferred,
}

pub struct GuestResource {
    pub(crate) handle: Mutex<Option<ResourceAny>>,
    pub(crate) drops: Weak<Mutex<Vec<ResourceAny>>>,
}

impl Drop for GuestResource {
    fn drop(&mut self) {
        if let (Some(any), Some(queue)) = (
            self.handle.lock().ok().and_then(|mut h| h.take()),
            self.drops.upgrade(),
        ) && any.owned()
            && let Ok(mut queue) = queue.lock()
        {
            queue.push(any);
        }
        // If the caller is gone the handle leaks until store teardown —
        // documented; there is no store access inside `Drop`.
    }
}

// ---------------------------------------------------------------------------
// Typed collection (one `ScriptBind` pass)
// ---------------------------------------------------------------------------

type FieldGet<U> = Box<dyn Fn(&U) -> Sv + Send + Sync>;
type FieldSet<U> = Box<dyn Fn(&mut U, Sv) -> Result<(), Sce> + Send + Sync>;
type CowFn<U> = for<'a> fn(ScriptCow<'a, U>, &[Sv]) -> Result<Sv, Ce>;
type MutFn<U> = fn(&mut U, &[Sv]) -> Result<Sv, Ce>;
type AsyncCowFn<U> = for<'a> fn(ScriptCow<'a, U>, &'a [Sv]) -> ScriptCallFuture<'a>;
type AsyncMutFn<U> = for<'a> fn(&'a mut U, &'a [Sv]) -> ScriptCallFuture<'a>;
type CtorFn<U> = fn(&[Sv]) -> Result<U, Ce>;
type AsyncCtorFn<U> = for<'a> fn(&'a [Sv]) -> ScriptCtorFuture<'a, U>;
type PropGetFn<U> = fn(&U) -> Sv;
type PropSetFn<U> = fn(&mut U, Sv) -> Result<(), Sce>;
type AsyncPropGetFn<U> = for<'a> fn(ScriptCow<'a, U>) -> ScriptCallFuture<'a>;
type AsyncPropSetFn<U> = for<'a> fn(&'a mut U, Sv) -> ScriptCallFuture<'a>;
type ScalarFn<U> = fn(U, &[Sv]) -> Result<U, Sce>;
type IndexFn<U> = fn(&U, &[Sv]) -> Result<Sv, Sce>;
type NewIndexFn<U> = fn(&mut U, &[Sv]) -> Result<(), Sce>;

type FieldEntry<U> = (&'static str, FieldGet<U>, Option<FieldSet<U>>);
type BinSelfEntry<U> = (&'static str, fn(U, U) -> U);

/// Dispatch key for one statically dispatched generic-method monomorph: the
/// method name plus the instantiation's type arguments.
pub(crate) type GenericKey = (&'static str, &'static [TypeDescriptor<'static>]);

/// Dispatch key for one `dyn`-dispatched generic-method candidate: the full
/// descriptor (the dispatcher ranks candidates against its parameters), the
/// candidate's type arguments, and the SELF type's instantiation (its
/// parameters substitute concretely — they are fixed per monomorph, so only
/// the method's own parameters become dispatcher cases).
#[cfg(feature = "dyn-generics")]
pub(crate) type DynKey = (
    &'static haphe::FunctionDescriptor<'static>,
    &'static [TypeDescriptor<'static>],
    haphe::SelfInstantiation,
);

/// Everything one `ScriptBind::bind` pass hands a binder, in typed form.
pub(crate) struct RawTable<U> {
    pub fields: Vec<FieldEntry<U>>,
    pub ctors: Vec<(&'static str, CtorFn<U>)>,
    pub ctors_async: Vec<(&'static str, AsyncCtorFn<U>)>,
    pub props_get: Vec<(&'static str, PropGetFn<U>)>,
    pub props_set: Vec<(&'static str, PropSetFn<U>)>,
    pub props_get_async: Vec<(&'static str, AsyncPropGetFn<U>)>,
    pub props_set_async: Vec<(&'static str, AsyncPropSetFn<U>)>,
    pub methods_cow: Vec<(&'static str, CowFn<U>)>,
    pub methods_mut: Vec<(&'static str, MutFn<U>)>,
    pub methods_async: Vec<(&'static str, AsyncCowFn<U>)>,
    pub methods_async_mut: Vec<(&'static str, AsyncMutFn<U>)>,
    pub methods_generic: Vec<(GenericKey, CowFn<U>)>,
    pub methods_generic_mut: Vec<(GenericKey, MutFn<U>)>,
    pub methods_generic_async: Vec<(GenericKey, AsyncCowFn<U>)>,
    pub methods_generic_async_mut: Vec<(GenericKey, AsyncMutFn<U>)>,
    #[cfg(feature = "dyn-generics")]
    pub methods_dyn: Vec<(DynKey, CowFn<U>)>,
    #[cfg(feature = "dyn-generics")]
    pub methods_dyn_mut: Vec<(DynKey, MutFn<U>)>,
    #[cfg(feature = "dyn-generics")]
    pub methods_dyn_async: Vec<(DynKey, AsyncCowFn<U>)>,
    #[cfg(feature = "dyn-generics")]
    pub methods_dyn_async_mut: Vec<(DynKey, AsyncMutFn<U>)>,
    pub tostring: Option<fn(&U) -> String>,
    pub concat: Option<fn(&U) -> String>,
    pub debug: Option<fn(&U) -> String>,
    pub hash: Option<fn(&U) -> u64>,
    pub eq: Option<fn(&U, &U) -> bool>,
    pub lt: Option<fn(&U, &U) -> bool>,
    pub le: Option<fn(&U, &U) -> bool>,
    pub unm: Option<fn(&U) -> U>,
    pub bnot: Option<fn(&U) -> U>,
    pub arith_self: Vec<BinSelfEntry<U>>,
    pub arith_scalar: Vec<(&'static str, &'static TypeDescriptor<'static>, ScalarFn<U>)>,
    pub iter: Option<fn(U) -> ScriptIter>,
    pub len: Option<fn(U) -> usize>,
    pub index: Option<IndexFn<U>>,
    pub newindex: Option<NewIndexFn<U>>,
    pub call: Option<IndexFn<U>>,
    pub call_async: Option<AsyncCowFn<U>>,
}

impl<U> Default for RawTable<U> {
    fn default() -> Self {
        Self {
            fields: Vec::new(),
            ctors: Vec::new(),
            ctors_async: Vec::new(),
            props_get: Vec::new(),
            props_set: Vec::new(),
            props_get_async: Vec::new(),
            props_set_async: Vec::new(),
            methods_cow: Vec::new(),
            methods_mut: Vec::new(),
            methods_async: Vec::new(),
            methods_async_mut: Vec::new(),
            methods_generic: Vec::new(),
            methods_generic_mut: Vec::new(),
            methods_generic_async: Vec::new(),
            methods_generic_async_mut: Vec::new(),
            #[cfg(feature = "dyn-generics")]
            methods_dyn: Vec::new(),
            #[cfg(feature = "dyn-generics")]
            methods_dyn_mut: Vec::new(),
            #[cfg(feature = "dyn-generics")]
            methods_dyn_async: Vec::new(),
            #[cfg(feature = "dyn-generics")]
            methods_dyn_async_mut: Vec::new(),
            tostring: None,
            concat: None,
            debug: None,
            hash: None,
            eq: None,
            lt: None,
            le: None,
            unm: None,
            bnot: None,
            arith_self: Vec::new(),
            arith_scalar: Vec::new(),
            iter: None,
            len: None,
            index: None,
            newindex: None,
            call: None,
            call_async: None,
        }
    }
}

impl<U: 'static> TypeBinder<U> for RawTable<U> {
    type Error = Infallible;

    fn field<V: IntoScript + FromScript + Clone + 'static>(
        &mut self,
        name: &'static str,
        getter: fn(&U) -> V,
        setter: Option<fn(&mut U, V)>,
    ) -> Result<(), Infallible> {
        let get: FieldGet<U> = Box::new(move |u| getter(u).into_script());
        let set: Option<FieldSet<U>> = setter.map(|s| {
            let f: FieldSet<U> = Box::new(move |u: &mut U, v: Sv| {
                s(u, V::from_script(v)?);
                Ok(())
            });
            f
        });
        self.fields.push((name, get, set));
        Ok(())
    }

    fn method(&mut self, name: &'static str, f: CowFn<U>) -> Result<(), Infallible> {
        self.methods_cow.push((name, f));
        Ok(())
    }

    fn method_mut(&mut self, name: &'static str, f: MutFn<U>) -> Result<(), Infallible> {
        self.methods_mut.push((name, f));
        Ok(())
    }

    fn method_async(&mut self, name: &'static str, f: AsyncCowFn<U>) -> Result<(), Infallible> {
        self.methods_async.push((name, f));
        Ok(())
    }

    fn method_async_mut(&mut self, name: &'static str, f: AsyncMutFn<U>) -> Result<(), Infallible> {
        self.methods_async_mut.push((name, f));
        Ok(())
    }

    fn method_generic(
        &mut self,
        name: &'static str,
        type_args: &'static [TypeDescriptor<'static>],
        f: CowFn<U>,
    ) -> Result<(), Infallible> {
        self.methods_generic.push(((name, type_args), f));
        Ok(())
    }

    fn method_generic_mut(
        &mut self,
        name: &'static str,
        type_args: &'static [TypeDescriptor<'static>],
        f: MutFn<U>,
    ) -> Result<(), Infallible> {
        self.methods_generic_mut.push(((name, type_args), f));
        Ok(())
    }

    fn method_generic_async(
        &mut self,
        name: &'static str,
        type_args: &'static [TypeDescriptor<'static>],
        f: AsyncCowFn<U>,
    ) -> Result<(), Infallible> {
        self.methods_generic_async.push(((name, type_args), f));
        Ok(())
    }

    fn method_generic_async_mut(
        &mut self,
        name: &'static str,
        type_args: &'static [TypeDescriptor<'static>],
        f: AsyncMutFn<U>,
    ) -> Result<(), Infallible> {
        self.methods_generic_async_mut.push(((name, type_args), f));
        Ok(())
    }

    // Dyn candidates are collected (never delegated to the plain channels,
    // which would collide monomorphs under one name); the synthesized
    // dispatcher serves them. Without the feature the core defaults stand,
    // safely: the capability check rejects dyn declarations before any bind.
    #[cfg(feature = "dyn-generics")]
    fn method_dyn(
        &mut self,
        descriptor: &'static haphe::FunctionDescriptor<'static>,
        type_args: &'static [TypeDescriptor<'static>],
        self_inst: haphe::SelfInstantiation,
        f: CowFn<U>,
    ) -> Result<(), Infallible> {
        self.methods_dyn
            .push(((descriptor, type_args, self_inst), f));
        Ok(())
    }

    #[cfg(feature = "dyn-generics")]
    fn method_dyn_mut(
        &mut self,
        descriptor: &'static haphe::FunctionDescriptor<'static>,
        type_args: &'static [TypeDescriptor<'static>],
        self_inst: haphe::SelfInstantiation,
        f: MutFn<U>,
    ) -> Result<(), Infallible> {
        self.methods_dyn_mut
            .push(((descriptor, type_args, self_inst), f));
        Ok(())
    }

    #[cfg(feature = "dyn-generics")]
    fn method_dyn_async(
        &mut self,
        descriptor: &'static haphe::FunctionDescriptor<'static>,
        type_args: &'static [TypeDescriptor<'static>],
        self_inst: haphe::SelfInstantiation,
        f: AsyncCowFn<U>,
    ) -> Result<(), Infallible> {
        self.methods_dyn_async
            .push(((descriptor, type_args, self_inst), f));
        Ok(())
    }

    #[cfg(feature = "dyn-generics")]
    fn method_dyn_async_mut(
        &mut self,
        descriptor: &'static haphe::FunctionDescriptor<'static>,
        type_args: &'static [TypeDescriptor<'static>],
        self_inst: haphe::SelfInstantiation,
        f: AsyncMutFn<U>,
    ) -> Result<(), Infallible> {
        self.methods_dyn_async_mut
            .push(((descriptor, type_args, self_inst), f));
        Ok(())
    }

    fn constructor(&mut self, name: &'static str, f: CtorFn<U>) -> Result<(), Infallible> {
        self.ctors.push((name, f));
        Ok(())
    }

    fn constructor_async(
        &mut self,
        name: &'static str,
        f: AsyncCtorFn<U>,
    ) -> Result<(), Infallible> {
        self.ctors_async.push((name, f));
        Ok(())
    }

    fn property_get(&mut self, name: &'static str, f: PropGetFn<U>) -> Result<(), Infallible> {
        self.props_get.push((name, f));
        Ok(())
    }

    fn property_set(&mut self, name: &'static str, f: PropSetFn<U>) -> Result<(), Infallible> {
        self.props_set.push((name, f));
        Ok(())
    }

    fn property_get_async(
        &mut self,
        name: &'static str,
        f: AsyncPropGetFn<U>,
    ) -> Result<(), Infallible> {
        self.props_get_async.push((name, f));
        Ok(())
    }

    fn property_set_async(
        &mut self,
        name: &'static str,
        f: AsyncPropSetFn<U>,
    ) -> Result<(), Infallible> {
        self.props_set_async.push((name, f));
        Ok(())
    }

    fn meta_tostring(&mut self, f: fn(&U) -> String) -> Result<(), Infallible> {
        self.tostring = Some(f);
        Ok(())
    }

    fn meta_concat(&mut self, f: fn(&U) -> String) -> Result<(), Infallible> {
        self.concat = Some(f);
        Ok(())
    }

    fn meta_hash(&mut self, f: fn(&U) -> u64) -> Result<(), Infallible> {
        self.hash = Some(f);
        Ok(())
    }

    fn meta_debug(&mut self, f: fn(&U) -> String) -> Result<(), Infallible> {
        self.debug = Some(f);
        Ok(())
    }

    fn meta_eq(&mut self, f: fn(&U, &U) -> bool) -> Result<(), Infallible> {
        self.eq = Some(f);
        Ok(())
    }

    fn meta_lt(&mut self, f: fn(&U, &U) -> bool) -> Result<(), Infallible> {
        self.lt = Some(f);
        Ok(())
    }

    fn meta_le(&mut self, f: fn(&U, &U) -> bool) -> Result<(), Infallible> {
        self.le = Some(f);
        Ok(())
    }

    fn meta_unm(&mut self, f: fn(&U) -> U) -> Result<(), Infallible> {
        self.unm = Some(f);
        Ok(())
    }

    fn meta_bnot(&mut self, f: fn(&U) -> U) -> Result<(), Infallible> {
        self.bnot = Some(f);
        Ok(())
    }

    fn meta_arith_self(&mut self, op: &'static str, f: fn(U, U) -> U) -> Result<(), Infallible> {
        self.arith_self.push((op, f));
        Ok(())
    }

    fn meta_arith_scalar(
        &mut self,
        op: &'static str,
        rhs: &'static TypeDescriptor<'static>,
        f: ScalarFn<U>,
    ) -> Result<(), Infallible> {
        self.arith_scalar.push((op, rhs, f));
        Ok(())
    }

    fn meta_iter(&mut self, f: fn(U) -> ScriptIter) -> Result<(), Infallible> {
        self.iter = Some(f);
        Ok(())
    }

    fn meta_len(&mut self, f: fn(U) -> usize) -> Result<(), Infallible> {
        self.len = Some(f);
        Ok(())
    }

    fn meta_call(&mut self, f: IndexFn<U>) -> Result<(), Infallible> {
        self.call = Some(f);
        Ok(())
    }

    fn meta_call_async(&mut self, f: AsyncCowFn<U>) -> Result<(), Infallible> {
        self.call_async = Some(f);
        Ok(())
    }

    fn meta_index(&mut self, f: IndexFn<U>) -> Result<(), Infallible> {
        self.index = Some(f);
        Ok(())
    }

    fn meta_newindex(&mut self, f: NewIndexFn<U>) -> Result<(), Infallible> {
        self.newindex = Some(f);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Erased resource entry (values live in the HostTable)
// ---------------------------------------------------------------------------

/// Receiver carrier at the erased layer, mirroring [`ScriptCow`].
pub(crate) enum CowAny<'a> {
    Borrowed(&'a (dyn Any + Send)),
    Owned(AnyBox),
}

type EGet = Box<dyn Fn(&(dyn Any + Send)) -> Result<Sv, Sce> + Send + Sync>;
type ESet = Box<dyn Fn(&mut (dyn Any + Send), Sv) -> Result<(), Sce> + Send + Sync>;
type ECow = Box<dyn for<'a> Fn(CowAny<'a>, &[Sv]) -> Result<Sv, Ce> + Send + Sync>;
type EMut = Box<dyn Fn(&mut (dyn Any + Send), &[Sv]) -> Result<Sv, Ce> + Send + Sync>;
type EAsyncCow = Box<dyn for<'a> Fn(CowAny<'a>, &'a [Sv]) -> ScriptCallFuture<'a> + Send + Sync>;
type EAsyncMut =
    Box<dyn for<'a> Fn(&'a mut (dyn Any + Send), &'a [Sv]) -> ScriptCallFuture<'a> + Send + Sync>;
type ECtor = Box<dyn Fn(&[Sv]) -> Result<AnyBox, Ce> + Send + Sync>;
type ERef<R> = Box<dyn Fn(&(dyn Any + Send)) -> R + Send + Sync>;
type EPair<R> = Box<dyn Fn(&(dyn Any + Send), &(dyn Any + Send)) -> R + Send + Sync>;
type EBinSelf = Box<dyn Fn(AnyBox, AnyBox) -> AnyBox + Send + Sync>;
type EScalar = Box<dyn Fn(AnyBox, &[Sv]) -> Result<AnyBox, Sce> + Send + Sync>;
type EIndex = Box<dyn Fn(&(dyn Any + Send), &[Sv]) -> Result<Sv, Sce> + Send + Sync>;
type ENewIndex = Box<dyn Fn(&mut (dyn Any + Send), &[Sv]) -> Result<(), Sce> + Send + Sync>;

type EAsyncGet = Box<dyn for<'a> Fn(CowAny<'a>) -> ScriptCallFuture<'a> + Send + Sync>;
type EAsyncSet =
    Box<dyn for<'a> Fn(&'a mut (dyn Any + Send), Sv) -> ScriptCallFuture<'a> + Send + Sync>;
type EAsyncCtorFut<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<AnyBox, Ce>> + 'a>>;
type EAsyncCtor = Box<dyn for<'a> Fn(&'a [Sv]) -> EAsyncCtorFut<'a> + Send + Sync>;

/// Erased computed-property accessors: any mix of sync and async halves.
#[derive(Default)]
pub(crate) struct EProp {
    pub get: Option<EGet>,
    pub set: Option<ESet>,
    pub get_async: Option<EAsyncGet>,
    pub set_async: Option<EAsyncSet>,
}

pub(crate) enum EMethod {
    Cow(ECow),
    Mut(EMut),
    AsyncCow(EAsyncCow),
    AsyncMut(EAsyncMut),
}

pub(crate) struct EField {
    pub get: EGet,
    pub set: Option<ESet>,
}

#[derive(Default)]
pub(crate) struct EMetas {
    pub tostring: Option<ERef<String>>,
    pub debug: Option<ERef<String>>,
    pub hash: Option<ERef<u64>>,
    pub eq: Option<EPair<bool>>,
    pub lt: Option<EPair<bool>>,
    pub le: Option<EPair<bool>>,
    pub unm: Option<ERef<AnyBox>>,
    pub bnot: Option<ERef<AnyBox>>,
    pub arith_self: HashMap<&'static str, EBinSelf>,
    pub arith_scalar: HashMap<&'static str, EScalar>,
    pub iter: Option<Box<dyn Fn(AnyBox) -> ScriptIter + Send + Sync>>,
    pub len: Option<Box<dyn Fn(AnyBox) -> usize + Send + Sync>>,
    pub index: Option<EIndex>,
    pub newindex: Option<ENewIndex>,
    pub call: Option<EIndex>,
    pub call_async: Option<EAsyncCow>,
}

fn default_metas() -> EMetas {
    EMetas::default()
}

/// Erased dispatch table for one registered resource type.
pub(crate) struct ResourceEntry {
    pub type_name: &'static str,
    pub fields: HashMap<&'static str, EField>,
    pub props: HashMap<&'static str, EProp>,
    pub ctors: HashMap<&'static str, ECtor>,
    pub ctors_async: HashMap<&'static str, EAsyncCtor>,
    pub methods: HashMap<&'static str, EMethod>,
    /// Statically dispatched generic-method monomorphs, one per declared
    /// instantiation, keyed by `(name, type_args)`.
    pub generic_methods: Vec<(GenericKey, EMethod)>,
    /// `dyn`-dispatched generic-method candidates (`dyn-generics` feature:
    /// the synthesized dispatcher scans them through the core resolver).
    #[cfg(feature = "dyn-generics")]
    pub dyn_methods: Vec<(DynKey, EMethod)>,
    pub metas: EMetas,
    /// Clones the live value into a fresh [`AnyBox`].
    pub clone_any: ECloneAny,
    /// Clones the live value into a `UserData` the derive-generated wrappers
    /// can `downcast` (an `Arc<U>` holding the concrete value).
    pub to_userdata: EToUserdata,
    /// Extracts a concrete value from a `UserData` (exact-type downcast).
    pub from_userdata: EFromUserdata,
}

type ECloneAny = Box<dyn Fn(&(dyn Any + Send)) -> AnyBox + Send + Sync>;
type EToUserdata = Box<dyn Fn(&(dyn Any + Send)) -> Sv + Send + Sync>;
type EFromUserdata = Box<dyn Fn(&Sv) -> Option<AnyBox> + Send + Sync>;

fn expect_u<U: 'static>(any: &(dyn Any + Send)) -> &U {
    any.downcast_ref::<U>()
        .expect("host table entry type_name was checked before dispatch")
}

fn expect_u_mut<U: 'static>(any: &mut (dyn Any + Send)) -> &mut U {
    any.downcast_mut::<U>()
        .expect("host table entry type_name was checked before dispatch")
}

fn expect_u_box<U: 'static>(any: AnyBox) -> U {
    *Box::<dyn Any>::downcast::<U>(any as Box<dyn Any>)
        .unwrap_or_else(|_| panic!("host table entry type_name was checked before dispatch"))
}

fn cow_of<U: 'static>(recv: CowAny<'_>) -> ScriptCow<'_, U> {
    match recv {
        CowAny::Borrowed(any) => ScriptCow::Borrowed(expect_u::<U>(any)),
        CowAny::Owned(any) => ScriptCow::Owned(expect_u_box::<U>(any)),
    }
}

/// Erases a collected [`RawTable`] into a [`ResourceEntry`] whose values
/// live in the host table.
#[allow(
    clippy::too_many_lines,
    reason = "one exhaustive pass per descriptor/channel shape; splitting the walk would scatter the per-shape rules"
)]
pub(crate) fn erase_resource<U>(type_name: &'static str, raw: RawTable<U>) -> ResourceEntry
where
    U: Clone + Send + Sync + 'static,
{
    let mut fields = HashMap::new();
    for (name, get, set) in raw.fields {
        fields.insert(
            name,
            EField {
                get: Box::new(move |any| Ok(get(expect_u::<U>(any)))),
                set: set.map(|s| {
                    let f: ESet = Box::new(move |any, v| s(expect_u_mut::<U>(any), v));
                    f
                }),
            },
        );
    }

    let mut ctors: HashMap<&'static str, ECtor> = HashMap::new();
    for (name, f) in raw.ctors {
        ctors.insert(name, Box::new(move |args| Ok(Box::new(f(args)?) as AnyBox)));
    }

    let mut ctors_async: HashMap<&'static str, EAsyncCtor> = HashMap::new();
    for (name, f) in raw.ctors_async {
        ctors_async.insert(
            name,
            Box::new(move |args| {
                Box::pin(async move { Ok(Box::new(f(args).await?) as AnyBox) }) as EAsyncCtorFut<'_>
            }),
        );
    }

    let mut props: HashMap<&'static str, EProp> = HashMap::new();
    for (name, f) in raw.props_get {
        props.entry(name).or_default().get = Some(Box::new(move |any| Ok(f(expect_u::<U>(any)))));
    }
    for (name, f) in raw.props_set {
        props.entry(name).or_default().set =
            Some(Box::new(move |any, v| f(expect_u_mut::<U>(any), v)));
    }
    for (name, f) in raw.props_get_async {
        props.entry(name).or_default().get_async = Some(Box::new(move |recv| f(cow_of::<U>(recv))));
    }
    for (name, f) in raw.props_set_async {
        props.entry(name).or_default().set_async =
            Some(Box::new(move |any, v| f(expect_u_mut::<U>(any), v)));
    }

    let mut methods: HashMap<&'static str, EMethod> = HashMap::new();
    for (name, f) in raw.methods_cow {
        methods.insert(
            name,
            EMethod::Cow(Box::new(move |recv, args| f(cow_of::<U>(recv), args))),
        );
    }
    for (name, f) in raw.methods_mut {
        methods.insert(
            name,
            EMethod::Mut(Box::new(move |any, args| f(expect_u_mut::<U>(any), args))),
        );
    }
    for (name, f) in raw.methods_async {
        methods.insert(
            name,
            EMethod::AsyncCow(Box::new(move |recv, args| f(cow_of::<U>(recv), args))),
        );
    }
    for (name, f) in raw.methods_async_mut {
        methods.insert(
            name,
            EMethod::AsyncMut(Box::new(move |any, args| f(expect_u_mut::<U>(any), args))),
        );
    }

    let mut generic_methods: Vec<(GenericKey, EMethod)> = Vec::new();
    for (key, f) in raw.methods_generic {
        generic_methods.push((
            key,
            EMethod::Cow(Box::new(move |recv, args| f(cow_of::<U>(recv), args))),
        ));
    }
    for (key, f) in raw.methods_generic_mut {
        generic_methods.push((
            key,
            EMethod::Mut(Box::new(move |any, args| f(expect_u_mut::<U>(any), args))),
        ));
    }
    for (key, f) in raw.methods_generic_async {
        generic_methods.push((
            key,
            EMethod::AsyncCow(Box::new(move |recv, args| f(cow_of::<U>(recv), args))),
        ));
    }
    for (key, f) in raw.methods_generic_async_mut {
        generic_methods.push((
            key,
            EMethod::AsyncMut(Box::new(move |any, args| f(expect_u_mut::<U>(any), args))),
        ));
    }

    #[cfg(feature = "dyn-generics")]
    let mut dyn_methods: Vec<(DynKey, EMethod)> = Vec::new();
    #[cfg(feature = "dyn-generics")]
    for (key, f) in raw.methods_dyn {
        dyn_methods.push((
            key,
            EMethod::Cow(Box::new(move |recv, args| f(cow_of::<U>(recv), args))),
        ));
    }
    #[cfg(feature = "dyn-generics")]
    for (key, f) in raw.methods_dyn_mut {
        dyn_methods.push((
            key,
            EMethod::Mut(Box::new(move |any, args| f(expect_u_mut::<U>(any), args))),
        ));
    }
    #[cfg(feature = "dyn-generics")]
    for (key, f) in raw.methods_dyn_async {
        dyn_methods.push((
            key,
            EMethod::AsyncCow(Box::new(move |recv, args| f(cow_of::<U>(recv), args))),
        ));
    }
    #[cfg(feature = "dyn-generics")]
    for (key, f) in raw.methods_dyn_async_mut {
        dyn_methods.push((
            key,
            EMethod::AsyncMut(Box::new(move |any, args| f(expect_u_mut::<U>(any), args))),
        ));
    }

    let mut metas = default_metas();
    if let Some(f) = raw.tostring {
        metas.tostring = Some(Box::new(move |any| f(expect_u::<U>(any))));
    }
    if let Some(f) = raw.debug {
        metas.debug = Some(Box::new(move |any| f(expect_u::<U>(any))));
    }
    if let Some(f) = raw.hash {
        metas.hash = Some(Box::new(move |any| f(expect_u::<U>(any))));
    }
    if let Some(f) = raw.eq {
        metas.eq = Some(Box::new(move |a, b| f(expect_u::<U>(a), expect_u::<U>(b))));
    }
    if let Some(f) = raw.lt {
        metas.lt = Some(Box::new(move |a, b| f(expect_u::<U>(a), expect_u::<U>(b))));
    }
    if let Some(f) = raw.le {
        metas.le = Some(Box::new(move |a, b| f(expect_u::<U>(a), expect_u::<U>(b))));
    }
    if let Some(f) = raw.unm {
        metas.unm = Some(Box::new(move |any| {
            Box::new(f(expect_u::<U>(any))) as AnyBox
        }));
    }
    if let Some(f) = raw.bnot {
        metas.bnot = Some(Box::new(move |any| {
            Box::new(f(expect_u::<U>(any))) as AnyBox
        }));
    }
    for (op, f) in raw.arith_self {
        metas.arith_self.insert(
            op,
            Box::new(move |a, b| Box::new(f(expect_u_box::<U>(a), expect_u_box::<U>(b))) as AnyBox),
        );
    }
    for (op, _rhs, f) in raw.arith_scalar {
        metas.arith_scalar.insert(
            op,
            Box::new(move |a, args| Ok(Box::new(f(expect_u_box::<U>(a), args)?) as AnyBox)),
        );
    }
    if let Some(f) = raw.iter {
        metas.iter = Some(Box::new(move |any| f(expect_u_box::<U>(any))));
    }
    if let Some(f) = raw.len {
        metas.len = Some(Box::new(move |any| f(expect_u_box::<U>(any))));
    }
    if let Some(f) = raw.index {
        metas.index = Some(Box::new(move |any, args| f(expect_u::<U>(any), args)));
    }
    if let Some(f) = raw.newindex {
        metas.newindex = Some(Box::new(move |any, args| f(expect_u_mut::<U>(any), args)));
    }
    if let Some(f) = raw.call {
        metas.call = Some(Box::new(move |any, args| f(expect_u::<U>(any), args)));
    }
    if let Some(f) = raw.call_async {
        metas.call_async = Some(Box::new(move |recv, args| f(cow_of::<U>(recv), args)));
    }

    ResourceEntry {
        type_name,
        fields,
        props,
        ctors,
        ctors_async,
        methods,
        generic_methods,
        #[cfg(feature = "dyn-generics")]
        dyn_methods,
        metas,
        clone_any: Box::new(|any| Box::new(expect_u::<U>(any).clone()) as AnyBox),
        to_userdata: Box::new(|any| Sv::UserData(OpaqueUserData::new(expect_u::<U>(any).clone()))),
        from_userdata: Box::new(|v| match v {
            Sv::UserData(ud) => ud
                .downcast_ref::<U>()
                .map(|u| Box::new(u.clone()) as AnyBox),
            _ => None,
        }),
    }
}

// ---------------------------------------------------------------------------
// Erased value entry (records and enums: value semantics, self by value)
// ---------------------------------------------------------------------------

type ESelfFn = Box<dyn Fn(Sv, &[Sv]) -> Result<Sv, Ce> + Send + Sync>;
type EValueCtor = Box<dyn Fn(&[Sv]) -> Result<Sv, Ce> + Send + Sync>;

/// Erased dispatch table for one registered value type (record or enum):
/// the receiver round-trips through `ScriptValue` conversions.
#[derive(Default)]
pub(crate) struct ValueEntry {
    /// Methods dispatchable with value semantics (`&self`, owned, static;
    /// async ones are driven to completion internally). `&mut self` methods
    /// are absent: value semantics cannot write back.
    pub methods: HashMap<&'static str, ESelfFn>,
    /// Statically dispatched generic-method monomorphs with value
    /// semantics, keyed by `(name, type_args)`. Like plain value methods,
    /// `&mut self` shapes are absent (no write-back channel).
    pub generic_methods: Vec<(GenericKey, ESelfFn)>,
    /// `dyn`-dispatched generic-method candidates with value semantics
    /// (`dyn-generics` feature). `&mut self` shapes are absent, like the
    /// static monomorphs.
    #[cfg(feature = "dyn-generics")]
    pub dyn_methods: Vec<(DynKey, ESelfFn)>,
    pub ctors: HashMap<&'static str, EValueCtor>,
    /// Trait projections, keyed by projected member source name. Each takes
    /// the receiver (and, where applicable, further args) as `ScriptValue`s:
    /// `args[0]` is always the receiver.
    pub projections: HashMap<&'static str, EValueCtor>,
    /// Computed-property getters with value semantics: `args[0]` is the
    /// receiver, the property value comes back.
    pub props_get: HashMap<&'static str, EValueCtor>,
    /// Computed-property setters with value semantics: `args[0]` receiver,
    /// `args[1]` value; the UPDATED record comes back (mirroring the
    /// `IndexSet` projection — a record cannot write back in place).
    pub props_set: HashMap<&'static str, EValueCtor>,
}

fn from_sv<U: FromScript>(v: &Sv) -> Result<U, Sce> {
    U::from_script(v.clone())
}

/// Erases a collected [`RawTable`] into a [`ValueEntry`]. The receiver is
/// rebuilt from its lowered `ScriptValue` per call; mutating projections
/// return the updated value.
///
/// Property and constructor channels (sync or async) are ignored here by
/// construction: a struct with any of them classifies as a RESOURCE, and
/// enums reject constructors/properties at derive time — so a value-erased
/// type can never carry them.
#[allow(
    clippy::too_many_lines,
    reason = "one exhaustive pass per descriptor/channel shape; splitting the walk would scatter the per-shape rules"
)]
pub(crate) fn erase_value<U>(raw: RawTable<U>) -> ValueEntry
where
    U: FromScript + IntoScript + Clone + Send + 'static,
{
    // Projections: receiver is args[0].
    fn this(args: &[Sv]) -> Result<&Sv, Sce> {
        args.first().ok_or(Sce {
            expected: "a receiver argument",
            got: "no arguments",
        })
    }
    let mut entry = ValueEntry::default();

    for (name, f) in raw.methods_cow {
        entry.methods.insert(
            name,
            Box::new(move |this, args| f(ScriptCow::Owned(from_sv::<U>(&this)?), args)),
        );
    }
    for (name, f) in raw.methods_async {
        entry.methods.insert(
            name,
            Box::new(move |this, args| {
                let u: U = from_sv(&this)?;
                block_on(f(ScriptCow::Owned(u), args))
            }),
        );
    }
    for ((name, args), f) in raw.methods_generic {
        entry.generic_methods.push((
            (name, args),
            Box::new(move |this, a| f(ScriptCow::Owned(from_sv::<U>(&this)?), a)),
        ));
    }
    for ((name, args), f) in raw.methods_generic_async {
        entry.generic_methods.push((
            (name, args),
            Box::new(move |this, a| {
                let u: U = from_sv(&this)?;
                block_on(f(ScriptCow::Owned(u), a))
            }),
        ));
    }
    #[cfg(feature = "dyn-generics")]
    for (key, f) in raw.methods_dyn {
        entry.dyn_methods.push((
            key,
            Box::new(move |this, a| f(ScriptCow::Owned(from_sv::<U>(&this)?), a)),
        ));
    }
    #[cfg(feature = "dyn-generics")]
    for (key, f) in raw.methods_dyn_async {
        entry.dyn_methods.push((
            key,
            Box::new(move |this, a| {
                let u: U = from_sv(&this)?;
                block_on(f(ScriptCow::Owned(u), a))
            }),
        ));
    }
    // `&mut self` methods (sync or async, generic included) are deliberately
    // not inserted: the companion-function shape has no way to hand the
    // mutated value back, so dispatch would silently lose the write. They
    // trap with a descriptive message instead (see runtime.rs).

    for (name, f) in raw.ctors {
        entry
            .ctors
            .insert(name, Box::new(move |args| Ok(f(args)?.into_script())));
    }

    // Computed properties: value semantics — getters return the property,
    // setters return the UPDATED record (no write-back channel exists).
    for (name, f) in raw.props_get {
        entry.props_get.insert(
            name,
            Box::new(move |args| {
                let u: U = from_sv(args.first().ok_or(Sce {
                    expected: "a receiver argument",
                    got: "no arguments",
                })?)?;
                Ok(f(&u))
            }),
        );
    }
    for (name, f) in raw.props_get_async {
        entry.props_get.insert(
            name,
            Box::new(move |args| {
                let u: U = from_sv(args.first().ok_or(Sce {
                    expected: "a receiver argument",
                    got: "no arguments",
                })?)?;
                block_on(f(ScriptCow::Owned(u)))
            }),
        );
    }
    for (name, f) in raw.props_set {
        entry.props_set.insert(
            name,
            Box::new(move |args| {
                let mut u: U = from_sv(args.first().ok_or(Sce {
                    expected: "a receiver argument",
                    got: "no arguments",
                })?)?;
                let value = args.get(1).cloned().ok_or(Sce {
                    expected: "a property value argument",
                    got: "no value",
                })?;
                f(&mut u, value)?;
                Ok(u.into_script())
            }),
        );
    }
    for (name, f) in raw.props_set_async {
        entry.props_set.insert(
            name,
            Box::new(move |args| {
                let mut u: U = from_sv(args.first().ok_or(Sce {
                    expected: "a receiver argument",
                    got: "no arguments",
                })?)?;
                let value = args.get(1).cloned().ok_or(Sce {
                    expected: "a property value argument",
                    got: "no value",
                })?;
                block_on(f(&mut u, value))?;
                Ok(u.into_script())
            }),
        );
    }

    if let Some(f) = raw.eq {
        entry.projections.insert(
            "eq",
            Box::new(move |args| {
                let a: U = from_sv(this(args)?)?;
                let b: U = from_sv(args.get(1).unwrap_or(&Sv::Unit))?;
                Ok(Sv::Bool(f(&a, &b)))
            }),
        );
    }
    if let Some(f) = raw.lt {
        entry.projections.insert(
            "lt",
            Box::new(move |args| {
                let a: U = from_sv(this(args)?)?;
                let b: U = from_sv(args.get(1).unwrap_or(&Sv::Unit))?;
                Ok(Sv::Bool(f(&a, &b)))
            }),
        );
    }
    if let Some(f) = raw.le {
        entry.projections.insert(
            "le",
            Box::new(move |args| {
                let a: U = from_sv(this(args)?)?;
                let b: U = from_sv(args.get(1).unwrap_or(&Sv::Unit))?;
                Ok(Sv::Bool(f(&a, &b)))
            }),
        );
    }
    if let Some(f) = raw.tostring.or(raw.concat) {
        entry.projections.insert(
            "to_string",
            Box::new(move |args| Ok(Sv::String(f(&from_sv::<U>(this(args)?)?)))),
        );
    }
    if let Some(f) = raw.debug {
        entry.projections.insert(
            "to_debug_string",
            Box::new(move |args| Ok(Sv::String(f(&from_sv::<U>(this(args)?)?)))),
        );
    }
    if let Some(f) = raw.hash {
        entry.projections.insert(
            "hash",
            Box::new(move |args| Ok(Sv::I64(f(&from_sv::<U>(this(args)?)?) as i64))),
        );
    }
    if let Some(f) = raw.unm {
        entry.projections.insert(
            "neg",
            Box::new(move |args| Ok(f(&from_sv::<U>(this(args)?)?).into_script())),
        );
    }
    if let Some(f) = raw.bnot {
        entry.projections.insert(
            "not",
            Box::new(move |args| Ok(f(&from_sv::<U>(this(args)?)?).into_script())),
        );
    }
    for (op, f) in raw.arith_self {
        entry.projections.insert(
            op,
            Box::new(move |args| {
                let a: U = from_sv(this(args)?)?;
                let b: U = from_sv(args.get(1).unwrap_or(&Sv::Unit))?;
                Ok(f(a, b).into_script())
            }),
        );
    }
    for (op, _rhs, f) in raw.arith_scalar {
        entry.projections.insert(
            op,
            Box::new(move |args| {
                let a: U = from_sv(this(args)?)?;
                Ok(f(a, &args[1..])?.into_script())
            }),
        );
    }
    if let Some(f) = raw.iter {
        entry.projections.insert(
            "items",
            Box::new(move |args| {
                let u: U = from_sv(this(args)?)?;
                Ok(Sv::List(f(u).collect()))
            }),
        );
    }
    if let Some(f) = raw.len {
        entry.projections.insert(
            "length",
            Box::new(move |args| Ok(Sv::I64(f(from_sv::<U>(this(args)?)?) as i64))),
        );
    }
    if let Some(f) = raw.index {
        entry.projections.insert(
            "at",
            Box::new(move |args| Ok(f(&from_sv::<U>(this(args)?)?, &args[1..])?)),
        );
    }
    if let Some(f) = raw.newindex {
        entry.projections.insert(
            "set_at",
            Box::new(move |args| {
                // Value semantics: mutate a rebuilt receiver and hand the
                // updated value back.
                let mut u: U = from_sv(this(args)?)?;
                f(&mut u, &args[1..])?;
                Ok(u.into_script())
            }),
        );
    }
    if let Some(f) = raw.call {
        entry.projections.insert(
            "call",
            Box::new(move |args| Ok(f(&from_sv::<U>(this(args)?)?, &args[1..])?)),
        );
    }
    if let Some(f) = raw.call_async {
        entry.projections.insert(
            "call",
            Box::new(move |args| {
                let u: U = from_sv(this(args)?)?;
                block_on(f(ScriptCow::Owned(u), &args[1..]))
            }),
        );
    }

    entry
}

// ---------------------------------------------------------------------------
// On-thread executor for the non-Send bridge futures
// ---------------------------------------------------------------------------

/// Drives a (non-`Send`) future to completion on the current thread with a
/// parking waker. Used for async provided dispatch, where wasmtime's dynamic
/// host functions are synchronous: the calling thread blocks until the
/// bridge future resolves.
///
/// Trade-offs (documented): the calling fiber/thread blocks for the whole
/// call; futures that need a reactor must be driven by a runtime living on
/// other threads (a tokio `current_thread` runtime would deadlock).
pub(crate) fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    use std::task::{Context, Poll, Wake, Waker};

    struct Signal {
        thread: std::thread::Thread,
        notified: AtomicBool,
    }

    impl Wake for Signal {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.notified.store(true, Ordering::SeqCst);
            self.thread.unpark();
        }
    }

    let signal = Arc::new(Signal {
        thread: std::thread::current(),
        notified: AtomicBool::new(false),
    });
    let waker = Waker::from(Arc::clone(&signal));
    let mut cx = Context::from_waker(&waker);

    let mut fut = std::pin::pin!(fut);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(out) => return out,
            Poll::Pending => {
                while !signal.notified.swap(false, Ordering::SeqCst) {
                    std::thread::park();
                }
            }
        }
    }
}

/// Renders a wrapper-channel error into a trap message: conversion failures
/// like [`trap_convert`], Host errors as their kind-prefixed rendering.
pub(crate) fn trap_call(name: &str, e: Ce) -> wasmtime::Error {
    match e {
        ScriptCallError::Convert(e) => trap_convert(name, &e),
        host @ ScriptCallError::Host { .. } => {
            wasmtime::Error::msg(format!("haphe-wit: `{name}`: {host}"))
        }
    }
}

/// Renders a `ScriptConvertError` chain into a trap message.
pub(crate) fn trap_convert(name: &str, e: &Sce) -> wasmtime::Error {
    let mut msg = format!("haphe-wit: `{name}`: ");
    let _ = write!(msg, "argument/result conversion failed: {e}");
    wasmtime::Error::msg(msg)
}
