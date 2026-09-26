//! Foreign direction: `#[script(foreign)]` trait handles dispatching into a
//! Rhai callbacks map.

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

use std::collections::HashMap;
use std::sync::Arc;

use haphe::{ForeignError, Script, script};
use haphe_rhai::{RhaiBindError, foreign_handle};
use rhai::{Dynamic, Engine, FnPtr};

/// Host-side hooks supplied by a Rhai script.
#[script(foreign, thread_safety = none)]
pub trait Hooks {
    fn greet(&self, name: String) -> String;

    fn add(&self, a: i64, b: i64) -> i64;

    fn maybe(&self, value: Option<i64>) -> Option<i64>;

    fn nums(&self) -> Vec<i64>;

    fn stats(&self) -> HashMap<String, i64>;

    #[script(rename = "shout")]
    fn make_loud(&self, text: String) -> String;

    fn checked(&self, n: i64) -> Result<i64, HookError>;

    fn pick(&self, color: Color) -> Color;
}

#[derive(Debug)]
pub struct HookError(String);

impl From<ForeignError> for HookError {
    fn from(e: ForeignError) -> Self {
        Self(e.to_string())
    }
}

/// A named color.
#[derive(Script, Debug, PartialEq)]
pub enum Color {
    Red,
    DarkBlue,
}

const CALLBACKS: &str = r#"
fn greet(name) { "hi " + name }
fn add(a, b) { a + b }
fn maybe(v) { v }
fn nums() { [1, 2, 3] }
fn stats() { #{ wins: 3, losses: 1 } }
fn shout(text) { text.to_upper() }
fn checked(n) {
    if n < 0 { throw "negative input"; }
    n * 2
}
fn pick(color) {
    if color == "Red" { "DarkBlue" } else { "Red" }
}
"#;

fn hooks_from(script: &str) -> Result<HooksHandle, RhaiBindError> {
    let engine = Engine::new();
    let ast = engine.compile(script).unwrap();

    let mut map = rhai::Map::new();
    for f in ast.iter_functions() {
        map.insert(f.name.into(), Dynamic::from(FnPtr::new(f.name).unwrap()));
    }

    foreign_handle(Arc::new(engine), &map, ast)
}

#[test]
fn callbacks_dispatch_with_shape_coverage() {
    let hooks = hooks_from(CALLBACKS).unwrap();

    assert_eq!(hooks.greet("ada".to_string()), "hi ada");
    assert_eq!(hooks.add(2, 40), 42);
    assert_eq!(hooks.maybe(Some(7)), Some(7));
    assert_eq!(hooks.maybe(None), None);
    assert_eq!(hooks.nums(), vec![1, 2, 3]);
    let stats = hooks.stats();
    assert_eq!(stats.get("wins"), Some(&3));
    assert_eq!(stats.get("losses"), Some(&1));
}

#[test]
fn renamed_method_is_keyed_by_exposed_name() {
    let hooks = hooks_from(CALLBACKS).unwrap();
    assert_eq!(hooks.make_loud("hey".to_string()), "HEY");
}

#[test]
fn unit_enums_round_trip_as_case_names() {
    let hooks = hooks_from(CALLBACKS).unwrap();
    assert_eq!(hooks.pick(Color::Red), Color::DarkBlue);
    assert_eq!(hooks.pick(Color::DarkBlue), Color::Red);
}

#[test]
fn rhai_error_surfaces_through_result_methods() {
    let hooks = hooks_from(CALLBACKS).unwrap();
    assert_eq!(hooks.checked(21).unwrap(), 42);
    let HookError(message) = hooks.checked(-1).unwrap_err();
    assert!(message.contains("checked"), "got: {message}");
    assert!(message.contains("negative input"), "got: {message}");
}

#[test]
#[should_panic(expected = "foreign function `add` failed")]
fn rhai_error_panics_through_non_result_methods() {
    let hooks = hooks_from(
        r#"
        fn greet(n) { n }
        fn add() { throw "boom"; }
        fn maybe(v) { v }
        fn nums() { [] }
        fn stats() { #{} }
        fn shout(t) { t }
        fn checked(n) { n }
        fn pick(c) { c }
        "#,
    )
    .unwrap();
    hooks.add(1, 1);
}

#[test]
fn missing_callback_is_a_build_time_error() {
    let engine = Engine::new();
    let ast = engine.compile("fn greet(n) { n }").unwrap();
    let mut map = rhai::Map::new();
    map.insert(
        "greet".into(),
        Dynamic::from(FnPtr::new("greet").unwrap()),
    );

    let Err(err) = foreign_handle::<HooksHandle>(Arc::new(engine), &map, ast) else {
        panic!("expected an error");
    };
    match err {
        RhaiBindError::MissingForeignFunction {
            interface,
            function,
        } => {
            assert_eq!(interface, "Hooks");
            assert_eq!(function, "add");
        }
        other => panic!("expected MissingForeignFunction, got: {other}"),
    }
}

// ---------------------------------------------------------------------------
// Generic foreign interfaces and methods
// ---------------------------------------------------------------------------

/// A host-backed store, generic over the value type.
#[script(foreign, thread_safety = none)]
pub trait Store<T> {
    fn get(&self, key: String) -> Option<T>;
}

/// Converts raw values, generic per method (STATIC dispatch: one Rhai
/// handler per declared instantiation, addressed by the mangled name).
#[script(foreign, thread_safety = none)]
pub trait Conv {
    #[script(instantiate(i64), instantiate(String))]
    fn convert<U>(&self, raw: String) -> U;

    #[script(instantiate(i64))]
    fn parse<U>(&self, raw: String) -> Result<U, HookError>;
}

/// Renders values, generic per method (`dyn`: one erased Rhai handler under
/// the plain name serves every instantiation).
#[script(foreign, thread_safety = none)]
pub trait Render {
    #[script(dyn, instantiate(i64), instantiate(String))]
    fn show<U>(&self, value: U) -> String;
}

const GENERIC_CALLBACKS: &str = r#"
fn get(key) {
    if key == "n" { 7 }
    else if key == "s" { "seven" }
    else { () }
}
fn convert__i64(raw) { parse_int(raw) }
fn convert__string(raw) { raw + "!" }
fn parse__i64(raw) { parse_int(raw) }
fn show(value) { "" + value }
"#;

fn generic_ast_and_map() -> (Engine, rhai::AST, rhai::Map) {
    let engine = Engine::new();
    let ast = engine.compile(GENERIC_CALLBACKS).unwrap();
    let mut map = rhai::Map::new();
    for f in ast.iter_functions() {
        map.insert(f.name.into(), Dynamic::from(FnPtr::new(f.name).unwrap()));
    }
    (engine, ast, map)
}

#[cfg(not(feature = "generics"))]
#[test]
fn generic_foreign_interface_rejected_without_feature() {
    let (engine, ast, map) = generic_ast_and_map();
    let Err(err) = foreign_handle::<StoreHandle<i64>>(Arc::new(engine), &map, ast) else {
        panic!("expected an error");
    };
    assert!(
        matches!(err, RhaiBindError::GenericForeignInterface { name: "Store" }),
        "got: {err}"
    );
}

#[cfg(not(feature = "generics"))]
#[test]
fn generic_foreign_method_rejected_without_feature() {
    let (engine, ast, map) = generic_ast_and_map();
    let Err(err) = foreign_handle::<ConvHandle>(Arc::new(engine), &map, ast) else {
        panic!("expected an error");
    };
    assert!(
        matches!(err, RhaiBindError::GenericFunction { name: "convert" }),
        "got: {err}"
    );
}

#[cfg(feature = "generics")]
#[test]
fn generic_interfaces_dispatch_dynamically() {
    let (engine, ast, map) = generic_ast_and_map();
    let engine = Arc::new(engine);
    let ints: StoreHandle<i64> =
        foreign_handle(Arc::clone(&engine), &map, ast.clone()).unwrap();
    let strings: StoreHandle<String> =
        foreign_handle(engine, &map, ast).unwrap();
    assert_eq!(ints.get("n".to_string()), Some(7));
    assert_eq!(strings.get("s".to_string()), Some("seven".to_string()));
    assert_eq!(ints.get("missing".to_string()), None);
}

#[cfg(feature = "generics")]
#[test]
fn static_generic_methods_route_to_mangled_handlers() {
    let (engine, ast, map) = generic_ast_and_map();
    let conv: ConvHandle = foreign_handle(Arc::new(engine), &map, ast).unwrap();
    let n: i64 = conv.convert("41".to_string());
    assert_eq!(n, 41);
    let s: String = conv.convert("x".to_string());
    assert_eq!(s, "x!");
}

#[cfg(feature = "generics")]
#[test]
fn undeclared_static_instantiation_errors_with_declared_set() {
    let (engine, ast, map) = generic_ast_and_map();
    let conv: ConvHandle = foreign_handle(Arc::new(engine), &map, ast).unwrap();
    let HookError(message) = conv.parse::<bool>("true".to_string()).unwrap_err();
    assert!(
        message.contains("no handler for instantiation `parse__bool`"),
        "got: {message}"
    );
    assert!(
        message.contains("declared instantiations (parse__i64)"),
        "got: {message}"
    );
}

#[cfg(feature = "generics")]
#[test]
fn missing_mangled_handler_is_a_build_time_error() {
    let engine = rhai::Engine::new();
    let ast = engine
        .compile(
            r"
            fn convert__i64(raw) { parse_int(raw) }
            fn parse__i64(raw) { parse_int(raw) }
            fn show(value) { `${value}` }
            ",
        )
        .unwrap();
    let mut map = rhai::Map::new();
    for f in ast.iter_functions() {
        map.insert(f.name.into(), Dynamic::from(FnPtr::new(f.name).unwrap()));
    }

    let Err(err) = foreign_handle::<ConvHandle>(Arc::new(engine), &map, ast) else {
        panic!("expected an error");
    };
    match err {
        RhaiBindError::MissingForeignInstantiation {
            interface,
            function,
            mangled,
        } => {
            assert_eq!(interface, "Conv");
            assert_eq!(function, "convert");
            assert_eq!(mangled, "convert__string");
        }
        other => panic!("expected MissingForeignInstantiation, got: {other}"),
    }
}

#[cfg(feature = "generics")]
#[test]
fn dyn_generic_methods_dispatch_through_one_handler() {
    let (engine, ast, map) = generic_ast_and_map();
    let render: RenderHandle = foreign_handle(Arc::new(engine), &map, ast).unwrap();
    assert_eq!(render.show(7_i64), "7");
    assert_eq!(render.show("x".to_string()), "x");
}

// ---------------------------------------------------------------------------
// Round trip: Rhai script → Rust method → Rhai foreign callback → Rust
// ---------------------------------------------------------------------------

/// A bound service whose methods dispatch through a foreign handle,
/// exercising the full Rhai→Rust→Rhai→Rust round-trip.
#[derive(Script, Clone)]
#[script(thread_safety = none, methods)]
struct Greeter {
    prefix: String,
    #[script(skip)]
    hooks: std::sync::Arc<HooksHandle>,
}

#[script]
impl Greeter {
    fn greet(&self, name: String) -> String {
        let raw = self.hooks.greet(name);
        format!("{} {raw}", self.prefix)
    }

    fn sum(&self, a: i64, b: i64) -> i64 {
        self.hooks.add(a, b)
    }
}

#[cfg(not(feature = "sync"))]
#[test]
fn round_trip_method_dispatches_through_foreign_callback() {
    let engine = Engine::new();
    let ast = engine.compile(CALLBACKS).unwrap();
    let mut map = rhai::Map::new();
    for f in ast.iter_functions() {
        map.insert(f.name.into(), Dynamic::from(FnPtr::new(f.name).unwrap()));
    }
    let hooks: HooksHandle = foreign_handle(Arc::new(engine), &map, ast).unwrap();

    let mut engine = Engine::new();
    let mut type_mod = rhai::Module::new();
    haphe_rhai::bind_type::<Greeter>(&mut engine, &mut type_mod).expect("binds");

    let mut scope = rhai::Scope::new();
    scope.push(
        "g",
        Greeter {
            prefix: ">>".to_string(),
            hooks: std::sync::Arc::new(hooks),
        },
    );

    // Rhai → Rust Greeter::greet → Rhai fn greet → Rust return → Rhai result
    let result: String = engine
        .eval_with_scope(&mut scope, r#"g.greet("ada")"#)
        .expect("eval");
    assert_eq!(result, ">> hi ada");

    // Same path through a different method
    let result: i64 = engine
        .eval_with_scope(&mut scope, "g.sum(19, 23)")
        .expect("eval");
    assert_eq!(result, 42);
}

// ---------------------------------------------------------------------------
// Round trip with generics: static and dyn dispatch through foreign handles
// ---------------------------------------------------------------------------

#[cfg(all(feature = "generics", not(feature = "sync")))]
mod round_trip_generics {
    use super::*;

    /// Bound type that dispatches through `Conv` (static generic foreign)
    /// and `Render` (dyn generic foreign).
    #[derive(Script, Clone)]
    #[script(thread_safety = none, methods)]
    struct Converter {
        tag: String,
        #[script(skip)]
        conv: std::sync::Arc<ConvHandle>,
        #[script(skip)]
        render: std::sync::Arc<RenderHandle>,
    }

    #[script]
    impl Converter {
        fn convert_int(&self, raw: String) -> i64 {
            self.conv.convert(raw)
        }

        fn convert_str(&self, raw: String) -> String {
            self.conv.convert(raw)
        }

        fn show_int(&self, value: i64) -> String {
            let rendered = self.render.show(value);
            format!("[{}] {rendered}", self.tag)
        }

        fn show_str(&self, value: String) -> String {
            let rendered = self.render.show(value);
            format!("[{}] {rendered}", self.tag)
        }
    }

    fn build_converter() -> (rhai::Engine, Converter) {
        let (cb_engine, ast, map) = super::generic_ast_and_map();
        let cb_engine = Arc::new(cb_engine);
        let conv: ConvHandle =
            foreign_handle(Arc::clone(&cb_engine), &map, ast.clone()).unwrap();
        let render: RenderHandle =
            foreign_handle(cb_engine, &map, ast).unwrap();

        let mut engine = Engine::new();
        let mut type_mod = rhai::Module::new();
        haphe_rhai::bind_type::<Converter>(&mut engine, &mut type_mod)
            .expect("binds");

        let converter = Converter {
            tag: "out".to_string(),
            conv: std::sync::Arc::new(conv),
            render: std::sync::Arc::new(render),
        };

        (engine, converter)
    }

    #[test]
    fn round_trip_static_generic_foreign_dispatch() {
        let (engine, converter) = build_converter();
        let mut scope = rhai::Scope::new();
        scope.push("c", converter);

        let n: i64 = engine
            .eval_with_scope(&mut scope, r#"c.convert_int("41")"#)
            .expect("eval");
        assert_eq!(n, 41);

        let s: String = engine
            .eval_with_scope(&mut scope, r#"c.convert_str("x")"#)
            .expect("eval");
        assert_eq!(s, "x!");
    }

    #[test]
    fn round_trip_dyn_generic_foreign_dispatch() {
        let (engine, converter) = build_converter();
        let mut scope = rhai::Scope::new();
        scope.push("c", converter);

        let out: String = engine
            .eval_with_scope(&mut scope, "c.show_int(42)")
            .expect("eval");
        assert_eq!(out, "[out] 42");

        let out: String = engine
            .eval_with_scope(&mut scope, r#"c.show_str("hello")"#)
            .expect("eval");
        assert_eq!(out, "[out] hello");
    }
}
