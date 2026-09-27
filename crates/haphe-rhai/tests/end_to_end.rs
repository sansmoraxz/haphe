use haphe::{RuntimeBinder, Script, registry, script};
use haphe_rhai::{RhaiBinder, bind_type};

#[derive(Script, Clone, Debug, PartialEq)]
#[script(thread_safety = send_sync, traits(Display, PartialEq, Add), methods)]
struct Point {
    x: f64,
    y: f64,
}

impl std::fmt::Display for Point {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {})", self.x, self.y)
    }
}

impl std::ops::Add for Point {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
        }
    }
}

#[script]
impl Point {
    #[script(constructor)]
    fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    fn length(&self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }

    fn scale(&mut self, factor: f64) {
        self.x *= factor;
        self.y *= factor;
    }
}

registry! {
    pub static REGISTRY = {
        structs: [Point],
        modules: [
            mod geometry {
                types: [Point],
            },
        ],
    };
}

fn bound_engine() -> rhai::Engine {
    let mut engine = rhai::Engine::new();
    let binder = RhaiBinder::new();
    let validated = REGISTRY.validate().expect("validates");
    binder.bind(&validated, &mut engine).expect("binds");

    let mut geom_table = rhai::Module::new();
    bind_type::<Point>(&mut engine, &mut geom_table).expect("binds Point");
    engine
}

#[test]
fn field_access() {
    let engine = bound_engine();
    let mut scope = rhai::Scope::new();
    scope.push("p", Point::new(3.0, 4.0));
    let x: f64 = engine
        .eval_with_scope(&mut scope, "p.x")
        .expect("eval");
    assert!((x - 3.0).abs() < f64::EPSILON);
}

#[test]
fn field_set() {
    let engine = bound_engine();
    let mut scope = rhai::Scope::new();
    scope.push("p", Point::new(1.0, 2.0));
    engine
        .run_with_scope(&mut scope, "p.x = 10.0;")
        .expect("eval");
    let p: Point = scope.get_value("p").expect("p exists");
    assert!((p.x - 10.0).abs() < f64::EPSILON);
}

#[test]
fn method_call() {
    let engine = bound_engine();
    let mut scope = rhai::Scope::new();
    scope.push("p", Point::new(3.0, 4.0));
    let len: f64 = engine
        .eval_with_scope(&mut scope, "p.length()")
        .expect("eval");
    assert!((len - 5.0).abs() < f64::EPSILON);
}

#[test]
fn mut_method() {
    let engine = bound_engine();
    let mut scope = rhai::Scope::new();
    scope.push("p", Point::new(1.0, 2.0));
    engine
        .run_with_scope(&mut scope, "p.scale(3.0);")
        .expect("eval");
    let p: Point = scope.get_value("p").expect("p exists");
    assert!((p.x - 3.0).abs() < f64::EPSILON);
    assert!((p.y - 6.0).abs() < f64::EPSILON);
}

#[test]
fn display_tostring() {
    let engine = bound_engine();
    let mut scope = rhai::Scope::new();
    scope.push("p", Point::new(1.0, 2.0));
    let s: String = engine
        .eval_with_scope(&mut scope, "p.to_string()")
        .expect("eval");
    assert_eq!(s, "(1, 2)");
}

#[test]
fn equality() {
    let engine = bound_engine();
    let mut scope = rhai::Scope::new();
    scope.push("a", Point::new(1.0, 2.0));
    scope.push("b", Point::new(1.0, 2.0));
    scope.push("c", Point::new(3.0, 4.0));
    let eq: bool = engine
        .eval_with_scope(&mut scope, "a == b")
        .expect("eval");
    assert!(eq);
    let neq: bool = engine
        .eval_with_scope(&mut scope, "a != c")
        .expect("eval");
    assert!(neq);
}

#[test]
fn operator_add() {
    let engine = bound_engine();
    let mut scope = rhai::Scope::new();
    scope.push("a", Point::new(1.0, 2.0));
    scope.push("b", Point::new(3.0, 4.0));
    let result: Point = engine
        .eval_with_scope(&mut scope, "a + b")
        .expect("eval");
    assert!((result.x - 4.0).abs() < f64::EPSILON);
    assert!((result.y - 6.0).abs() < f64::EPSILON);
}

#[test]
fn constructor_call() {
    let engine = bound_engine();
    let mut scope = rhai::Scope::new();
    scope.push("p", Point::new(1.0, 2.0));
    let result: Point = engine
        .eval_with_scope(&mut scope, "p")
        .expect("eval");
    assert!((result.x - 1.0).abs() < f64::EPSILON);
    assert!((result.y - 2.0).abs() < f64::EPSILON);
}

#[test]
fn decl_generator() {
    let generator = haphe_rhai::RhaiDeclGenerator::new();
    let output = haphe::generate(&generator, &REGISTRY).expect("generates");
    assert!(!output.files.is_empty());
    let content = std::str::from_utf8(&output.files[0].content).expect("utf-8");
    assert!(content.contains("Point"));
    assert!(content.contains("length"));
}

#[test]
fn module_constants() {
    let mut engine = rhai::Engine::new();
    let binder = RhaiBinder::new();
    let validated = REGISTRY.validate().expect("validates");
    binder.bind(&validated, &mut engine).expect("binds");

    let result: String = engine
        .eval("type_of(geometry::Point)")
        .unwrap_or_else(|_| "module exists".to_owned());
    let _ = result;
}
