//! `#[script]` on impl blocks: methods, constructors, properties, async,
//! renames, ownership — matched against hand-written descriptors.

#![cfg(feature = "macros")]

use haphe::{
    FunctionDescriptor, Ownership, ParamDescriptor, PrimitiveType, PropertyDescriptor, Receiver,
    Script, ScriptImpl, ScriptStruct, ThreadSafety, TypeDescriptor, TypeId, script,
};

/// A 2D point.
#[derive(Script)]
#[script(thread_safety = send_sync, methods)]
struct Point {
    x: f64,
    y: f64,
}

#[script]
impl Point {
    /// Creates a point.
    #[script(constructor)]
    pub fn new(x: f64, y: f64) -> Self {
        Point { x, y }
    }

    pub fn distance_to(&self, other: &Point) -> f64 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }

    pub fn scaled(self, #[script(clone)] factor: f64) -> Point {
        Point {
            x: self.x * factor,
            y: self.y * factor,
        }
    }

    #[script(rename = "fetch", error_kind = "IOError")]
    #[allow(dead_code)]
    pub async fn fetch_data(&self) -> Result<String, String> {
        Ok(format!("{},{}", self.x, self.y))
    }

    /// The point's distance from the origin.
    #[script(getter)]
    pub fn length(&self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }

    #[script(setter)]
    pub fn set_length(&mut self, value: f64) {
        let scale = value / self.length();
        self.x *= scale;
        self.y *= scale;
    }

    #[script(skip)]
    fn internal(&self) -> f64 {
        self.x
    }
}

const F64: TypeDescriptor<'static> = TypeDescriptor::Primitive(PrimitiveType::F64);
const POINT_REF: TypeDescriptor<'static> = TypeDescriptor::Ref(TypeId::new("script_impl::Point"));

static EXPECTED_METHODS: &[FunctionDescriptor<'static>] = &[
    FunctionDescriptor {
        name: "distance_to",
        doc: None,
        receiver: Some(Receiver::Ref),
        params: &[ParamDescriptor {
            name: "other",
            ty: &POINT_REF,
            ownership: Ownership::Ref,
        }],
        return_type: &F64,
        return_ownership: Ownership::Owned,
        is_async: false,
        error_kind: None,
    },
    FunctionDescriptor {
        name: "scaled",
        doc: None,
        receiver: Some(Receiver::Owned),
        params: &[ParamDescriptor {
            name: "factor",
            ty: &F64,
            ownership: Ownership::Clone,
        }],
        return_type: &POINT_REF,
        return_ownership: Ownership::Owned,
        is_async: false,
        error_kind: None,
    },
    FunctionDescriptor {
        name: "fetch",
        doc: None,
        receiver: Some(Receiver::Ref),
        params: &[],
        return_type: &TypeDescriptor::Result(&TypeDescriptor::String, &TypeDescriptor::String),
        return_ownership: Ownership::Owned,
        is_async: true,
        error_kind: Some("IOError"),
    },
];

static EXPECTED_CONSTRUCTORS: &[FunctionDescriptor<'static>] = &[FunctionDescriptor {
    name: "new",
    doc: Some("Creates a point."),
    receiver: None,
    params: &[
        ParamDescriptor {
            name: "x",
            ty: &F64,
            ownership: Ownership::Owned,
        },
        ParamDescriptor {
            name: "y",
            ty: &F64,
            ownership: Ownership::Owned,
        },
    ],
    return_type: &POINT_REF,
    return_ownership: Ownership::Owned,
    is_async: false,
    error_kind: None,
}];

static EXPECTED_PROPERTIES: &[PropertyDescriptor<'static>] = &[PropertyDescriptor {
    name: "length",
    doc: Some("The point's distance from the origin."),
    ty: &F64,
    readonly: false,
}];

#[test]
fn methods_match_hand_written() {
    assert_eq!(<Point as ScriptImpl>::METHODS, EXPECTED_METHODS);
    assert_eq!(<Point as ScriptImpl>::CONSTRUCTORS, EXPECTED_CONSTRUCTORS);
    assert_eq!(<Point as ScriptImpl>::PROPERTIES, EXPECTED_PROPERTIES);
    const { assert!(<Point as ScriptImpl>::HAS_ASYNC) };
}

#[test]
fn descriptor_references_impl_block() {
    let desc = <Point as ScriptStruct>::DESCRIPTOR;
    assert_eq!(desc.methods, EXPECTED_METHODS);
    assert_eq!(desc.constructors, EXPECTED_CONSTRUCTORS);
    assert_eq!(desc.properties, EXPECTED_PROPERTIES);
    assert_eq!(desc.thread_safety, ThreadSafety::SEND_SYNC);
}

/// The functions themselves still work as ordinary Rust.
#[test]
fn functions_are_untouched() {
    let mut p = Point::new(3.0, 4.0);
    assert_eq!(p.length(), 5.0);
    assert_eq!(p.distance_to(&Point::new(0.0, 0.0)), 5.0);
    p.set_length(10.0);
    assert_eq!(p.length(), 10.0);
    assert_eq!(p.internal(), 6.0);
    assert_eq!(p.scaled(0.5).x, 3.0);
}

/// A readonly property: getter without a paired setter.
#[derive(Script)]
#[script(methods)]
struct Circle {
    radius: f64,
}

#[script]
impl Circle {
    #[script(getter)]
    pub fn area(&self) -> f64 {
        std::f64::consts::PI * self.radius * self.radius
    }
}

#[test]
fn getter_only_property_is_readonly() {
    let props = <Circle as ScriptImpl>::PROPERTIES;
    assert_eq!(props.len(), 1);
    assert!(props[0].readonly);
    const { assert!(!<Circle as ScriptImpl>::HAS_ASYNC) };
    let _ = Circle { radius: 1.0 }.area();
}

/// `#[cfg]`-gated methods are described only when compiled in, and skipped
/// functions may keep parameter attributes.
#[derive(Script)]
#[script(methods)]
#[allow(dead_code)]
struct Gated {
    n: i64,
}

#[script]
#[allow(dead_code)]
impl Gated {
    fn always(&self) -> i64 {
        self.n
    }

    #[cfg(any())]
    fn sometimes(&self) -> i64 {
        self.n
    }

    // A cfg'd-out async method must not force a thread-safety declaration.
    #[cfg(any())]
    async fn sometimes_async(&self) -> i64 {
        self.n
    }

    #[script(skip)]
    #[allow(dead_code)]
    fn hidden(&self, #[script(clone)] factor: f64) -> f64 {
        factor
    }
}

#[test]
fn cfg_gated_methods_follow_compilation() {
    let names: Vec<&str> = <Gated as ScriptImpl>::METHODS
        .iter()
        .map(|m| m.name)
        .collect();
    assert_eq!(names, ["always"]);
    const { assert!(!<Gated as ScriptImpl>::HAS_ASYNC) };
}

/// An `&str` getter pairs with a `String` setter: both describe the same
/// script type.
#[derive(Script)]
#[script(methods)]
struct Named {
    name: String,
}

#[script]
impl Named {
    /// The display name.
    #[script(getter)]
    fn name(&self) -> &str {
        &self.name
    }

    #[script(setter)]
    fn set_name(&mut self, value: String) {
        self.name = value;
    }
}

#[test]
fn str_getter_pairs_with_string_setter() {
    let props = <Named as ScriptImpl>::PROPERTIES;
    assert_eq!(props.len(), 1);
    assert!(!props[0].readonly);
    assert_eq!(*props[0].ty, TypeDescriptor::String);
    let mut n = Named { name: "a".into() };
    n.set_name("b".into());
    assert_eq!(n.name(), "b");
}

/// Raw identifiers are exposed unrawed.
#[derive(Script)]
#[script(methods)]
struct Config {
    r#type: String,
}

#[script]
impl Config {
    fn r#match(&self) -> bool {
        !self.r#type.is_empty()
    }
}

#[test]
fn raw_identifiers_are_unrawed() {
    assert_eq!(<Config as ScriptStruct>::DESCRIPTOR.fields[0].name, "type");
    assert_eq!(<Config as ScriptImpl>::METHODS[0].name, "match");
    assert!(Config { r#type: "t".into() }.r#match());
}

/// Fallible constructors return `Result<Self, E>`.
#[derive(Script)]
#[script(methods)]
struct Port {
    number: u16,
}

#[script]
impl Port {
    #[script(constructor, error_kind = "ValueError")]
    fn new(number: i64) -> Result<Self, String> {
        u16::try_from(number)
            .map(|number| Port { number })
            .map_err(|e| e.to_string())
    }
}

#[test]
fn fallible_constructor() {
    let ctor = &<Port as ScriptImpl>::CONSTRUCTORS[0];
    assert_eq!(ctor.error_kind, Some("ValueError"));
    assert!(matches!(*ctor.return_type, TypeDescriptor::Result(..)));
    assert!(Port::new(70000).is_err());
    assert_eq!(Port::new(80).unwrap().number, 80);
}

/// `Self` in accessor signatures is substituted before verification.
#[derive(Script)]
#[script(methods)]
#[allow(dead_code)]
struct Node {
    twin: Option<Box<Node>>,
}

#[script]
#[allow(dead_code)]
impl Node {
    #[script(getter)]
    fn twin(&self) -> Option<Node> {
        None
    }

    #[script(setter)]
    fn set_twin(&mut self, value: Option<Self>) {
        self.twin = value.map(Box::new);
    }
}

#[test]
fn self_in_accessor_signatures() {
    let props = <Node as ScriptImpl>::PROPERTIES;
    assert_eq!(props.len(), 1);
    assert!(!props[0].readonly);
}
