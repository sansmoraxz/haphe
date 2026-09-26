//! Associated fns (no receiver) bind through the dedicated `associated*`
//! channels: `splat(21)` needs no instance.

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

#[derive(Script, Clone)]
#[script(thread_safety = send_sync, methods)]
struct Meter {
    reading: i64,
}

#[script]
impl Meter {
    #[script(constructor)]
    fn new(reading: i64) -> Self {
        Meter { reading }
    }

    fn read(&self) -> i64 {
        self.reading
    }

    fn splat(seed: i64) -> i64 {
        seed * 2
    }

    #[script(error_kind = "ParseError")]
    fn parse(text: String) -> Result<i64, TextError> {
        text.trim()
            .parse::<i64>()
            .map_err(|e| TextError(e.to_string()))
    }
}

fn env() -> rhai::Engine {
    let mut engine = rhai::Engine::new();
    let mut type_mod = rhai::Module::new();
    bind_type::<Meter>(&mut engine, &mut type_mod).expect("binds");
    engine
}

#[test]
fn associated_fn_called_without_instance() {
    let engine = env();
    let mut scope = rhai::Scope::new();
    let out: i64 = engine
        .eval_with_scope(&mut scope, "splat(21)")
        .unwrap();
    assert_eq!(out, 42);
}

#[test]
fn fallible_associated_fn_ok_and_err() {
    let engine = env();
    let mut scope = rhai::Scope::new();
    let out: i64 = engine
        .eval_with_scope(&mut scope, r#"parse(" 7 ")"#)
        .unwrap();
    assert_eq!(out, 7);
    let err = engine
        .eval_with_scope::<i64>(&mut scope, r#"parse("x")"#)
        .unwrap_err();
    assert!(err.to_string().contains("ParseError"), "{err}");
}

// ---------------------------------------------------------------------------
// Generic associated functions
// ---------------------------------------------------------------------------

#[cfg(feature = "generics")]
mod with_generics {
    use super::*;

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

    #[derive(Script, Clone)]
    #[script(thread_safety = send_sync, methods)]
    struct Tally {
        base: i64,
    }

    #[script]
    impl Tally {
        #[script(instantiate(i64), instantiate(String))]
        fn first<T>(a: T, b: T) -> T {
            let _ = b;
            a
        }

        #[script(dyn, instantiate(i64), instantiate(f64))]
        fn total<T: Acc>(value: T) -> i64 {
            value.acc()
        }
    }

    #[test]
    fn generic_associated_fns_bind_without_error() {
        let mut engine = rhai::Engine::new();
        let mut type_mod = rhai::Module::new();
        bind_type::<Tally>(&mut engine, &mut type_mod)
            .expect("generics-bearing type binds");
    }
}

#[cfg(not(feature = "generics"))]
mod without_generics {
    use super::*;

    #[derive(Script, Clone)]
    #[script(thread_safety = send_sync, methods)]
    struct Gen {
        n: i64,
    }

    #[script]
    impl Gen {
        #[script(instantiate(i64))]
        fn pick<T>(value: T) -> T {
            value
        }
    }

    #[test]
    fn generic_associated_fn_rejected_without_feature() {
        let mut engine = rhai::Engine::new();
        let mut type_mod = rhai::Module::new();
        let err = bind_type::<Gen>(&mut engine, &mut type_mod).unwrap_err();
        assert!(
            matches!(err, haphe_rhai::RhaiBindError::GenericFunction { name } if name == "pick"),
            "expected GenericFunction, got {err}"
        );
    }
}

#[derive(Debug)]
struct TextError(String);

impl std::fmt::Display for TextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TextError {}
