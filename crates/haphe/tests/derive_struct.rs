//! `#[derive(Script)]` on structs must produce exactly the descriptor a user
//! would write by hand.

#![cfg(feature = "macros")]

use std::cell::Cell;

use haphe::{
    FieldDescriptor, HapheType, PrimitiveType, Script, ScriptStruct, ScriptType, StructDescriptor,
    ThreadSafety, TraitImpl, TypeDescriptor, TypeId,
};

/// A 2D point.
///
/// Used by the geometry module.
#[derive(Script)]
#[script(thread_safety = send_sync, traits(Display, PartialEq, Add))]
struct Point {
    x: f64,
    /// Vertical coordinate.
    #[script(readonly, rename = "why")]
    y: f64,
    #[script(skip)]
    #[allow(dead_code)]
    cache: Option<f64>,
}

impl std::fmt::Display for Point {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {})", self.x, self.y)
    }
}

impl PartialEq for Point {
    fn eq(&self, other: &Self) -> bool {
        self.x == other.x && self.y == other.y
    }
}

impl std::ops::Add for Point {
    type Output = Point;
    fn add(self, rhs: Point) -> Point {
        Point {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
            cache: None,
        }
    }
}

const F64: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::F64);
const POINT_REF: TypeDescriptor<'static> = TypeDescriptor::Ref(TypeId::new("derive_struct::Point"));

static HAND_WRITTEN: StructDescriptor<'static> = StructDescriptor {
    id: TypeId::new("derive_struct::Point"),
    name: "Point",
    doc: Some("A 2D point.\n\nUsed by the geometry module."),
    fields: &[
        FieldDescriptor {
            name: "x",
            doc: None,
            ty: &F64,
            readonly: false,
        },
        FieldDescriptor {
            name: "why",
            doc: Some("Vertical coordinate."),
            ty: &F64,
            readonly: true,
        },
    ],
    methods: &[],
    constructors: &[],
    properties: &[],
    trait_impls: &[
        TraitImpl::Display,
        TraitImpl::PartialEq,
        TraitImpl::Add {
            rhs: &POINT_REF,
            output: &POINT_REF,
        },
    ],
    thread_safety: ThreadSafety::SEND_SYNC,
    generic_params: &[],
};

/// Evaluated in a `static` initializer: the const-construction guarantee.
static GENERATED: StructDescriptor<'static> = <Point as ScriptStruct>::DESCRIPTOR;

#[test]
fn generated_matches_hand_written() {
    assert_eq!(GENERATED, HAND_WRITTEN);
}

#[test]
fn haphe_type_resolves_to_ref() {
    assert_eq!(<Point as HapheType>::DESCRIPTOR, POINT_REF);
    assert_eq!(
        <Point as ScriptType>::ID,
        TypeId::new("derive_struct::Point")
    );
    // Nesting a derived type through the trait works.
    assert_eq!(
        <Vec<Point> as HapheType>::DESCRIPTOR,
        TypeDescriptor::List(&POINT_REF),
    );
}

/// Renaming changes `name` but never the `TypeId`.
#[derive(Script)]
#[script(rename = "Vec2", thread_safety = none)]
#[allow(dead_code)]
struct Vector2 {
    x: f64,
    y: f64,
}

#[test]
fn rename_keeps_type_id() {
    let desc = <Vector2 as ScriptStruct>::DESCRIPTOR;
    assert_eq!(desc.name, "Vec2");
    assert_eq!(desc.id, TypeId::new("derive_struct::Vector2"));
    assert_eq!(desc.thread_safety, ThreadSafety::NONE);
}

/// Default thread safety is `none` — the claim-nothing default.
#[derive(Script)]
#[allow(dead_code)]
struct Untouched {
    n: i32,
}

#[test]
fn default_thread_safety_is_none() {
    assert_eq!(
        <Untouched as ScriptStruct>::DESCRIPTOR.thread_safety,
        ThreadSafety::NONE
    );
}

/// `#[script(bytes)]` overrides `Vec<u8>` to the compact encoding.
#[derive(Script)]
#[allow(dead_code)]
struct Packet {
    #[script(bytes)]
    payload: Vec<u8>,
    /// Without the override, `Vec<u8>` is a plain list.
    raw: Vec<u8>,
}

#[test]
fn bytes_override() {
    let desc = <Packet as ScriptStruct>::DESCRIPTOR;
    assert_eq!(*desc.fields[0].ty, TypeDescriptor::Bytes);
    assert_eq!(
        *desc.fields[1].ty,
        TypeDescriptor::List(&TypeDescriptor::Primitive(PrimitiveType::U8)),
    );
}

/// A `!Sync` type can still be exposed with the default claim.
#[derive(Script)]
#[allow(dead_code)]
struct NotSync {
    counter: i32,
    #[script(skip)]
    #[allow(dead_code)]
    inner: Cell<i32>,
}

#[test]
fn not_sync_defaults_fine() {
    assert_eq!(
        <NotSync as ScriptStruct>::DESCRIPTOR.thread_safety,
        ThreadSafety::NONE
    );
}

/// Unit structs are valid marker types.
#[derive(Script)]
struct Marker;

#[test]
fn unit_struct() {
    assert!(<Marker as ScriptStruct>::DESCRIPTOR.fields.is_empty());
}

/// Explicit trait arguments naming `Self` — the documented spelling.
#[derive(Script)]
#[script(traits(Add(rhs = f64, output = Self)))]
struct Scalable {
    v: f64,
}

impl std::ops::Add<f64> for Scalable {
    type Output = Scalable;
    fn add(self, rhs: f64) -> Scalable {
        Scalable { v: self.v + rhs }
    }
}

#[test]
fn explicit_self_in_trait_args() {
    let desc = <Scalable as ScriptStruct>::DESCRIPTOR;
    let TraitImpl::Add { rhs, output } = desc.trait_impls[0] else {
        panic!("expected Add");
    };
    assert_eq!(*rhs, TypeDescriptor::Primitive(PrimitiveType::F64));
    assert_eq!(
        *output,
        TypeDescriptor::Ref(TypeId::new("derive_struct::Scalable"))
    );
}
