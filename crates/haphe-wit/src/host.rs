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
    FromScript, IntoScript, OpaqueUserData, ScriptCallFuture, ScriptConvertError, ScriptCow,
    ScriptIter, ScriptValue, TypeBinder, TypeDescriptor,
};
use wasmtime::component::ResourceAny;

type Sv = ScriptValue;
type Sce = ScriptConvertError;

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
type CowFn<U> = for<'a> fn(ScriptCow<'a, U>, &[Sv]) -> Result<Sv, Sce>;
type MutFn<U> = fn(&mut U, &[Sv]) -> Result<Sv, Sce>;
type AsyncCowFn<U> = for<'a> fn(ScriptCow<'a, U>, &'a [Sv]) -> ScriptCallFuture<'a>;
type AsyncMutFn<U> = for<'a> fn(&'a mut U, &'a [Sv]) -> ScriptCallFuture<'a>;
type CtorFn<U> = fn(&[Sv]) -> Result<U, Sce>;
type ScalarFn<U> = fn(U, &[Sv]) -> Result<U, Sce>;
type IndexFn<U> = fn(&U, &[Sv]) -> Result<Sv, Sce>;
type NewIndexFn<U> = fn(&mut U, &[Sv]) -> Result<(), Sce>;

type FieldEntry<U> = (&'static str, FieldGet<U>, Option<FieldSet<U>>);
type BinSelfEntry<U> = (&'static str, fn(U, U) -> U);

/// Everything one `ScriptBind::bind` pass hands a binder, in typed form.
pub(crate) struct RawTable<U> {
    pub fields: Vec<FieldEntry<U>>,
    pub ctors: Vec<(&'static str, CtorFn<U>)>,
    pub methods_cow: Vec<(&'static str, CowFn<U>)>,
    pub methods_mut: Vec<(&'static str, MutFn<U>)>,
    pub methods_async: Vec<(&'static str, AsyncCowFn<U>)>,
    pub methods_async_mut: Vec<(&'static str, AsyncMutFn<U>)>,
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
            methods_cow: Vec::new(),
            methods_mut: Vec::new(),
            methods_async: Vec::new(),
            methods_async_mut: Vec::new(),
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

    fn constructor(&mut self, name: &'static str, f: CtorFn<U>) -> Result<(), Infallible> {
        self.ctors.push((name, f));
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
type ECow = Box<dyn for<'a> Fn(CowAny<'a>, &[Sv]) -> Result<Sv, Sce> + Send + Sync>;
type EMut = Box<dyn Fn(&mut (dyn Any + Send), &[Sv]) -> Result<Sv, Sce> + Send + Sync>;
type EAsyncCow = Box<dyn for<'a> Fn(CowAny<'a>, &'a [Sv]) -> ScriptCallFuture<'a> + Send + Sync>;
type EAsyncMut =
    Box<dyn for<'a> Fn(&'a mut (dyn Any + Send), &'a [Sv]) -> ScriptCallFuture<'a> + Send + Sync>;
type ECtor = Box<dyn Fn(&[Sv]) -> Result<AnyBox, Sce> + Send + Sync>;
type ERef<R> = Box<dyn Fn(&(dyn Any + Send)) -> R + Send + Sync>;
type EPair<R> = Box<dyn Fn(&(dyn Any + Send), &(dyn Any + Send)) -> R + Send + Sync>;
type EBinSelf = Box<dyn Fn(AnyBox, AnyBox) -> AnyBox + Send + Sync>;
type EScalar = Box<dyn Fn(AnyBox, &[Sv]) -> Result<AnyBox, Sce> + Send + Sync>;
type EIndex = Box<dyn Fn(&(dyn Any + Send), &[Sv]) -> Result<Sv, Sce> + Send + Sync>;
type ENewIndex = Box<dyn Fn(&mut (dyn Any + Send), &[Sv]) -> Result<(), Sce> + Send + Sync>;

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
    pub ctors: HashMap<&'static str, ECtor>,
    pub methods: HashMap<&'static str, EMethod>,
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
        ctors,
        methods,
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

type ESelfFn = Box<dyn Fn(Sv, &[Sv]) -> Result<Sv, Sce> + Send + Sync>;
type EValueCtor = Box<dyn Fn(&[Sv]) -> Result<Sv, Sce> + Send + Sync>;

/// Erased dispatch table for one registered value type (record or enum):
/// the receiver round-trips through `ScriptValue` conversions.
#[derive(Default)]
pub(crate) struct ValueEntry {
    /// Methods dispatchable with value semantics (`&self`, owned, static;
    /// async ones are driven to completion internally). `&mut self` methods
    /// are absent: value semantics cannot write back.
    pub methods: HashMap<&'static str, ESelfFn>,
    pub ctors: HashMap<&'static str, EValueCtor>,
    /// Trait projections, keyed by projected member source name. Each takes
    /// the receiver (and, where applicable, further args) as `ScriptValue`s:
    /// `args[0]` is always the receiver.
    pub projections: HashMap<&'static str, EValueCtor>,
}

fn from_sv<U: FromScript>(v: &Sv) -> Result<U, Sce> {
    U::from_script(v.clone())
}

/// Erases a collected [`RawTable`] into a [`ValueEntry`]. The receiver is
/// rebuilt from its lowered `ScriptValue` per call; mutating projections
/// return the updated value.
pub(crate) fn erase_value<U>(raw: RawTable<U>) -> ValueEntry
where
    U: FromScript + IntoScript + Clone + Send + 'static,
{
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
    // `&mut self` methods (sync or async) are deliberately not inserted:
    // the companion-function shape has no way to hand the mutated value
    // back, so dispatch would silently lose the write. They trap with a
    // descriptive message instead (see runtime.rs).

    for (name, f) in raw.ctors {
        entry
            .ctors
            .insert(name, Box::new(move |args| Ok(f(args)?.into_script())));
    }

    // Projections: receiver is args[0].
    fn this(args: &[Sv]) -> Result<&Sv, Sce> {
        args.first().ok_or(Sce {
            expected: "a receiver argument",
            got: "no arguments",
        })
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
            Box::new(move |args| f(&from_sv::<U>(this(args)?)?, &args[1..])),
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
            Box::new(move |args| f(&from_sv::<U>(this(args)?)?, &args[1..])),
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
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

    struct Signal {
        thread: std::thread::Thread,
        notified: AtomicBool,
    }

    fn raw(signal: Arc<Signal>) -> RawWaker {
        RawWaker::new(Arc::into_raw(signal) as *const (), &VTABLE)
    }
    unsafe fn clone(data: *const ()) -> RawWaker {
        let signal = unsafe { Arc::from_raw(data as *const Signal) };
        let cloned = Arc::clone(&signal);
        std::mem::forget(signal);
        raw(cloned)
    }
    unsafe fn wake(data: *const ()) {
        let signal = unsafe { Arc::from_raw(data as *const Signal) };
        signal.notified.store(true, Ordering::SeqCst);
        signal.thread.unpark();
    }
    unsafe fn wake_by_ref(data: *const ()) {
        let signal = unsafe { &*(data as *const Signal) };
        signal.notified.store(true, Ordering::SeqCst);
        signal.thread.unpark();
    }
    unsafe fn drop_raw(data: *const ()) {
        drop(unsafe { Arc::from_raw(data as *const Signal) });
    }
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop_raw);

    let signal = Arc::new(Signal {
        thread: std::thread::current(),
        notified: AtomicBool::new(false),
    });
    let waker = unsafe { Waker::from_raw(raw(Arc::clone(&signal))) };
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

/// Renders a `ScriptConvertError` chain into a trap message.
pub(crate) fn trap_convert(name: &str, e: Sce) -> wasmtime::Error {
    let mut msg = format!("haphe-wit: `{name}`: ");
    let _ = write!(msg, "argument/result conversion failed: {e}");
    wasmtime::Error::msg(msg)
}
