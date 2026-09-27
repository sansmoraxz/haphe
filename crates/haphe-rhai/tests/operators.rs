//! Arithmetic and comparison operators: Sub, Mul/Div scalar, Neg, Not,
//! ordering, Pow, bitwise, Rem, and string concatenation.

#![allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    clippy::doc_markdown,
    clippy::trivially_copy_pass_by_ref,
    reason = "fixture shapes are dictated by the bridge surface under test: `#[script]` functions receive OWNED values (the boundary contract), methods keep their declared receivers (`&self` on Copy enums included), and docs name fixture idents verbatim"
)]
#![allow(
    dead_code,
    reason = "fixtures are exercised through their generated descriptors and bridge wrappers, not direct calls"
)]

use haphe::{Script, script};
use haphe_rhai::bind_type;

// ---------------------------------------------------------------------------
// Vec2 — arithmetic, comparison, unary, concat
// ---------------------------------------------------------------------------

#[derive(Script, Clone, Debug, PartialEq)]
#[script(
    thread_safety = send_sync,
    traits(
        Display, PartialEq, PartialOrd,
        Add, Sub,
        Mul(rhs = f64, output = Self),
        Div(rhs = f64, output = Self),
        Neg,
    ),
    methods,
)]
struct Vec2 {
    x: f64,
    y: f64,
}

impl std::fmt::Display for Vec2 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {})", self.x, self.y)
    }
}

impl PartialOrd for Vec2 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        let len_self = (self.x * self.x + self.y * self.y).sqrt();
        let len_other = (other.x * other.x + other.y * other.y).sqrt();
        len_self.partial_cmp(&len_other)
    }
}

impl std::ops::Add for Vec2 {
    type Output = Vec2;
    fn add(self, rhs: Vec2) -> Vec2 {
        Vec2 {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
        }
    }
}

impl std::ops::Sub for Vec2 {
    type Output = Vec2;
    fn sub(self, rhs: Vec2) -> Vec2 {
        Vec2 {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
        }
    }
}

impl std::ops::Mul<f64> for Vec2 {
    type Output = Vec2;
    fn mul(self, rhs: f64) -> Vec2 {
        Vec2 {
            x: self.x * rhs,
            y: self.y * rhs,
        }
    }
}

impl std::ops::Div<f64> for Vec2 {
    type Output = Vec2;
    fn div(self, rhs: f64) -> Vec2 {
        Vec2 {
            x: self.x / rhs,
            y: self.y / rhs,
        }
    }
}

impl std::ops::Neg for Vec2 {
    type Output = Vec2;
    fn neg(self) -> Vec2 {
        Vec2 {
            x: -self.x,
            y: -self.y,
        }
    }
}

#[script]
impl Vec2 {
    #[script(constructor)]
    fn new(x: f64, y: f64) -> Self {
        Vec2 { x, y }
    }
}

fn vec2_engine() -> rhai::Engine {
    let mut engine = rhai::Engine::new();
    let mut m = rhai::Module::new();
    bind_type::<Vec2>(&mut engine, &mut m).expect("binds Vec2");
    engine
}

// ---------------------------------------------------------------------------
// Mask — bitwise operators
// ---------------------------------------------------------------------------

#[derive(Script, Clone, Copy, Debug, PartialEq)]
#[script(thread_safety = send_sync, traits(PartialEq, BitAnd, BitOr, BitXor, Shl, Shr, Not), methods)]
struct Mask {
    bits: i64,
}

macro_rules! mask_binops {
    ($($trait:ident :: $method:ident => $op:tt),*) => {$(
        impl std::ops::$trait for Mask {
            type Output = Mask;
            fn $method(self, rhs: Mask) -> Mask {
                Mask { bits: self.bits $op rhs.bits }
            }
        }
    )*};
}
mask_binops!(BitAnd::bitand => &, BitOr::bitor => |, BitXor::bitxor => ^, Shl::shl => <<, Shr::shr => >>);

impl std::ops::Not for Mask {
    type Output = Mask;
    fn not(self) -> Mask {
        Mask { bits: !self.bits }
    }
}

#[script]
impl Mask {
    #[script(constructor)]
    fn new(bits: i64) -> Self {
        Mask { bits }
    }

    fn get(&self) -> i64 {
        self.bits
    }
}

fn mask_engine() -> rhai::Engine {
    let mut engine = rhai::Engine::new();
    let mut m = rhai::Module::new();
    bind_type::<Mask>(&mut engine, &mut m).expect("binds Mask");
    engine
}

// ---------------------------------------------------------------------------
// Scalar — Pow
// ---------------------------------------------------------------------------

#[derive(Script, Clone, Copy, Debug, PartialEq)]
#[script(thread_safety = send_sync, traits(PartialEq, Pow))]
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

fn scalar_engine() -> rhai::Engine {
    let mut engine = rhai::Engine::new();
    let mut m = rhai::Module::new();
    bind_type::<Scalar>(&mut engine, &mut m).expect("binds Scalar");
    engine
}

// ---------------------------------------------------------------------------
// Modular — Rem
// ---------------------------------------------------------------------------

#[derive(Script, Clone, Copy, Debug, PartialEq)]
#[script(thread_safety = send_sync, traits(PartialEq, Rem), methods)]
struct Modular {
    n: i64,
}

impl std::ops::Rem for Modular {
    type Output = Modular;
    fn rem(self, rhs: Modular) -> Modular {
        Modular {
            n: self.n % rhs.n,
        }
    }
}

#[script]
impl Modular {
    #[script(constructor)]
    fn new(n: i64) -> Self {
        Modular { n }
    }
}

fn modular_engine() -> rhai::Engine {
    let mut engine = rhai::Engine::new();
    let mut m = rhai::Module::new();
    bind_type::<Modular>(&mut engine, &mut m).expect("binds Modular");
    engine
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn sub_operator() {
    let engine = vec2_engine();
    let mut scope = rhai::Scope::new();
    scope.push("a", Vec2::new(5.0, 8.0));
    scope.push("b", Vec2::new(1.0, 3.0));
    let result: Vec2 = engine
        .eval_with_scope(&mut scope, "a - b")
        .expect("eval");
    assert!((result.x - 4.0).abs() < f64::EPSILON);
    assert!((result.y - 5.0).abs() < f64::EPSILON);
}

#[test]
fn mul_scalar_operator() {
    let engine = vec2_engine();
    let mut scope = rhai::Scope::new();
    scope.push("v", Vec2::new(1.0, 2.0));
    let result: Vec2 = engine
        .eval_with_scope(&mut scope, "v * 3.0")
        .expect("eval");
    assert!((result.x - 3.0).abs() < f64::EPSILON);
    assert!((result.y - 6.0).abs() < f64::EPSILON);
}

#[test]
fn div_scalar_operator() {
    let engine = vec2_engine();
    let mut scope = rhai::Scope::new();
    scope.push("v", Vec2::new(6.0, 10.0));
    let result: Vec2 = engine
        .eval_with_scope(&mut scope, "v / 2.0")
        .expect("eval");
    assert!((result.x - 3.0).abs() < f64::EPSILON);
    assert!((result.y - 5.0).abs() < f64::EPSILON);
}

#[test]
fn neg_unary() {
    let engine = vec2_engine();
    let mut scope = rhai::Scope::new();
    scope.push("v", Vec2::new(1.0, 2.0));
    let result: Vec2 = engine
        .eval_with_scope(&mut scope, "-v")
        .expect("eval");
    assert!((result.x - -1.0).abs() < f64::EPSILON);
    assert!((result.y - -2.0).abs() < f64::EPSILON);
}

#[test]
fn ordering_operators() {
    let engine = vec2_engine();
    let mut scope = rhai::Scope::new();
    scope.push("short", Vec2::new(1.0, 0.0));
    scope.push("long", Vec2::new(3.0, 4.0));

    let lt: bool = engine
        .eval_with_scope(&mut scope, "short < long")
        .expect("eval");
    assert!(lt);
    let le: bool = engine
        .eval_with_scope(&mut scope, "short <= long")
        .expect("eval");
    assert!(le);
    let gt: bool = engine
        .eval_with_scope(&mut scope, "long > short")
        .expect("eval");
    assert!(gt);
    let ge: bool = engine
        .eval_with_scope(&mut scope, "long >= short")
        .expect("eval");
    assert!(ge);
    let not_gt: bool = engine
        .eval_with_scope(&mut scope, "short > long")
        .expect("eval");
    assert!(!not_gt);
}

#[test]
fn not_unary() {
    let engine = mask_engine();
    let mut scope = rhai::Scope::new();
    scope.push("m", Mask::new(0b1100));
    let result: Mask = engine
        .eval_with_scope(&mut scope, "!m")
        .expect("eval");
    assert_eq!(result.bits, !0b1100_i64);
}

#[test]
fn bitwise_operators() {
    let engine = mask_engine();
    let mut scope = rhai::Scope::new();
    scope.push("a", Mask::new(0b1100));
    scope.push("b", Mask::new(0b1010));

    let and: Mask = engine
        .eval_with_scope(&mut scope, "a & b")
        .expect("eval &");
    assert_eq!(and.bits, 0b1000);

    let or: Mask = engine
        .eval_with_scope(&mut scope, "a | b")
        .expect("eval |");
    assert_eq!(or.bits, 0b1110);

    let xor: Mask = engine
        .eval_with_scope(&mut scope, "a ^ b")
        .expect("eval ^");
    assert_eq!(xor.bits, 0b0110);

    let mut scope2 = rhai::Scope::new();
    scope2.push("m", Mask::new(1));
    scope2.push("n", Mask::new(3));
    let shl: Mask = engine
        .eval_with_scope(&mut scope2, "m << n")
        .expect("eval <<");
    assert_eq!(shl.bits, 8);
    let shr: Mask = engine
        .eval_with_scope(&mut scope2, "n >> m")
        .expect("eval >>");
    assert_eq!(shr.bits, 1);
}

#[test]
fn pow_operator() {
    let engine = scalar_engine();
    let mut scope = rhai::Scope::new();
    scope.push("a", Scalar { value: 2.0 });
    scope.push("b", Scalar { value: 10.0 });
    let result: Scalar = engine
        .eval_with_scope(&mut scope, "a ** b")
        .expect("eval");
    assert!((result.value - 1024.0).abs() < f64::EPSILON);
}

#[test]
fn rem_operator() {
    let engine = modular_engine();
    let mut scope = rhai::Scope::new();
    scope.push("a", Modular::new(17));
    scope.push("b", Modular::new(5));
    let result: Modular = engine
        .eval_with_scope(&mut scope, "a % b")
        .expect("eval");
    assert_eq!(result.n, 2);
}

#[test]
fn concat_with_string() {
    let engine = vec2_engine();
    let mut scope = rhai::Scope::new();
    scope.push("v", Vec2::new(1.0, 2.0));
    let result: String = engine
        .eval_with_scope(&mut scope, r#"v + " suffix""#)
        .expect("eval");
    assert_eq!(result, "(1, 2) suffix");
}
