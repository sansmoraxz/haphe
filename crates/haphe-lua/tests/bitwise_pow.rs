//! Bitwise and pow operator bridging: `__pow` works on every Lua version;
//! the bitwise metamethods exist only on Lua 5.3+ and registration is
//! rejected descriptively elsewhere.

#![allow(dead_code)]

use haphe::Script;
use haphe_lua::bind_type;
use mlua::Lua;

// ---------------------------------------------------------------------------
// Pow — all versions
// ---------------------------------------------------------------------------

#[derive(Script, Clone, Copy, PartialEq, Debug)]
#[script(traits(Pow, PartialEq))]
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

#[test]
fn pow_operator_works_on_all_versions() {
    let lua = Lua::new();
    let tbl = lua.create_table().unwrap();
    bind_type::<Scalar>(&lua, &tbl).unwrap();
    lua.globals()
        .set("a", lua.create_any_userdata(Scalar { value: 2.0 }).unwrap())
        .unwrap();
    lua.globals()
        .set(
            "b",
            lua.create_any_userdata(Scalar { value: 10.0 }).unwrap(),
        )
        .unwrap();
    let out: mlua::AnyUserData = lua.load("return a ^ b").eval().unwrap();
    assert_eq!(*out.borrow::<Scalar>().unwrap(), Scalar { value: 1024.0 });
}

// ---------------------------------------------------------------------------
// Bitwise — declared once, bound on 5.3+, rejected elsewhere
// ---------------------------------------------------------------------------

#[derive(Script, Clone, Copy, PartialEq, Debug)]
#[script(traits(BitAnd, BitOr, BitXor, Shl, Shr, Not, PartialEq))]
struct Mask {
    bits: u32,
}

// Mask exercises the DEFAULT route end-to-end: direct `haphe::ops::*` impls
// through a real Lua VM.
macro_rules! mask_binops {
    ($($trait:ident :: $method:ident => $op:tt),*) => {$(
        impl haphe::ops::$trait for Mask {
            type Output = Mask;
            fn $method(self, rhs: Mask) -> Mask {
                Mask { bits: self.bits $op rhs.bits }
            }
        }
    )*};
}
mask_binops!(BitAnd::bitand => &, BitOr::bitor => |, BitXor::bitxor => ^, Shl::shl => <<, Shr::shr => >>);

impl haphe::ops::Not for Mask {
    type Output = Mask;
    fn not(self) -> Mask {
        Mask { bits: !self.bits }
    }
}

#[cfg(any(feature = "lua55", feature = "lua54", feature = "lua53"))]
mod on_lua53_plus {
    use super::*;

    fn mask_env(lua: &Lua) {
        let tbl = lua.create_table().unwrap();
        bind_type::<Mask>(lua, &tbl).unwrap();
        lua.globals()
            .set("a", lua.create_any_userdata(Mask { bits: 0b1100 }).unwrap())
            .unwrap();
        lua.globals()
            .set("b", lua.create_any_userdata(Mask { bits: 0b1010 }).unwrap())
            .unwrap();
        lua.globals()
            .set("one", lua.create_any_userdata(Mask { bits: 1 }).unwrap())
            .unwrap();
        lua.globals()
            .set("two", lua.create_any_userdata(Mask { bits: 2 }).unwrap())
            .unwrap();
    }

    fn eval_mask(lua: &Lua, code: &str) -> Mask {
        let out: mlua::AnyUserData = lua.load(code).eval().unwrap();
        *out.borrow::<Mask>().unwrap()
    }

    #[test]
    fn bitwise_operators_work() {
        let lua = Lua::new();
        mask_env(&lua);
        assert_eq!(eval_mask(&lua, "return a & b"), Mask { bits: 0b1000 });
        assert_eq!(eval_mask(&lua, "return a | b"), Mask { bits: 0b1110 });
        assert_eq!(eval_mask(&lua, "return a ~ b"), Mask { bits: 0b0110 });
        assert_eq!(eval_mask(&lua, "return a << one"), Mask { bits: 0b11000 });
        assert_eq!(eval_mask(&lua, "return a >> two"), Mask { bits: 0b11 });
        assert_eq!(eval_mask(&lua, "return ~a"), Mask { bits: !0b1100u32 });
    }

    // Scalar rhs flows through the same two-pass overload dispatch as
    // arithmetic scalars.
    #[derive(Script, Clone, Copy, PartialEq, Debug)]
    #[script(traits(Shl(rhs = u32, output = Self), PartialEq))]
    struct Shifty {
        bits: u32,
    }

    impl std::ops::Shl<u32> for Shifty {
        type Output = Shifty;
        fn shl(self, rhs: u32) -> Shifty {
            Shifty {
                bits: self.bits << rhs,
            }
        }
    }

    #[test]
    fn scalar_rhs_bitwise_works() {
        let lua = Lua::new();
        let tbl = lua.create_table().unwrap();
        bind_type::<Shifty>(&lua, &tbl).unwrap();
        lua.globals()
            .set("s", lua.create_any_userdata(Shifty { bits: 0b1 }).unwrap())
            .unwrap();
        let out: mlua::AnyUserData = lua.load("return s << 3").eval().unwrap();
        assert_eq!(*out.borrow::<Shifty>().unwrap(), Shifty { bits: 0b1000 });
    }
}

#[cfg(not(any(feature = "lua55", feature = "lua54", feature = "lua53")))]
mod pre_lua53 {
    use super::*;
    use haphe_lua::LuaBindError;

    #[test]
    fn bitwise_registration_rejected_descriptively() {
        let lua = Lua::new();
        let tbl = lua.create_table().unwrap();
        let Err(err) = bind_type::<Mask>(&lua, &tbl) else {
            panic!("binding a bitwise-declared type must fail before Lua 5.3");
        };
        let LuaBindError::UnsupportedOperator { op } = err else {
            panic!("expected UnsupportedOperator, got {err}");
        };
        assert!(["bitand", "bitor", "bitxor", "bnot", "shl", "shr"].contains(&op));
        assert!(err.to_string().contains("Lua 5.3+"));
    }

    #[test]
    fn pow_still_works_without_bitwise_support() {
        let lua = Lua::new();
        let tbl = lua.create_table().unwrap();
        bind_type::<Scalar>(&lua, &tbl).unwrap();
        lua.globals()
            .set("a", lua.create_any_userdata(Scalar { value: 3.0 }).unwrap())
            .unwrap();
        lua.globals()
            .set("b", lua.create_any_userdata(Scalar { value: 2.0 }).unwrap())
            .unwrap();
        let out: mlua::AnyUserData = lua.load("return a ^ b").eval().unwrap();
        assert_eq!(*out.borrow::<Scalar>().unwrap(), Scalar { value: 9.0 });
    }
}

// ---------------------------------------------------------------------------
// Floor division (`//`)
// ---------------------------------------------------------------------------

// Explicit IDiv: TRUE floor semantics, matching Lua's `//`.
#[derive(Script, Clone, Copy, PartialEq, Debug)]
#[script(traits(IDiv, PartialEq))]
struct Ratio {
    n: i64,
}

impl haphe::ops::IDiv for Ratio {
    type Output = Ratio;
    fn idiv(self, rhs: Ratio) -> Ratio {
        Ratio {
            n: self.n.div_euclid(rhs.n),
        }
    }
}

// Integer-scalar Div, no IDiv: eligible for the `//` fallback — which
// carries Rust's TRUNCATING semantics.
#[derive(Script, Clone, Copy, PartialEq, Debug)]
#[script(traits(Div(rhs = i64, output = Self), PartialEq))]
struct Truncy {
    n: i64,
}

impl std::ops::Div<i64> for Truncy {
    type Output = Truncy;
    fn div(self, rhs: i64) -> Truncy {
        Truncy { n: self.n / rhs }
    }
}

// Float-scalar Div: NOT eligible for the fallback.
#[derive(Script, Clone, Copy, PartialEq, Debug)]
#[script(traits(Div(rhs = f64, output = Self), PartialEq))]
struct Floaty {
    v: f64,
}

impl std::ops::Div<f64> for Floaty {
    type Output = Floaty;
    fn div(self, rhs: f64) -> Floaty {
        Floaty { v: self.v / rhs }
    }
}

#[cfg(any(feature = "lua55", feature = "lua54", feature = "lua53"))]
mod idiv_on_lua53_plus {
    use super::*;

    #[test]
    fn explicit_idiv_floors_negative_operands() {
        let lua = Lua::new();
        let tbl = lua.create_table().unwrap();
        bind_type::<Ratio>(&lua, &tbl).unwrap();
        lua.globals()
            .set("a", lua.create_any_userdata(Ratio { n: -7 }).unwrap())
            .unwrap();
        lua.globals()
            .set("b", lua.create_any_userdata(Ratio { n: 2 }).unwrap())
            .unwrap();
        let out: mlua::AnyUserData = lua.load("return a // b").eval().unwrap();
        // Floor semantics: -7 // 2 == -4, matching Lua's own `//`.
        assert_eq!(*out.borrow::<Ratio>().unwrap(), Ratio { n: -4 });
    }

    #[test]
    fn integer_div_fallback_truncates() {
        let lua = Lua::new();
        let tbl = lua.create_table().unwrap();
        bind_type::<Truncy>(&lua, &tbl).unwrap();
        lua.globals()
            .set("t", lua.create_any_userdata(Truncy { n: -7 }).unwrap())
            .unwrap();
        // `//` answers via the Div fallback — Rust's `/` truncates toward
        // zero, so this is -3, NOT Lua's floor result -4. Declare
        // traits(IDiv) for floor behavior; the fallback is a convenience
        // for types that only declared an integer Div.
        let out: mlua::AnyUserData = lua.load("return t // 2").eval().unwrap();
        assert_eq!(*out.borrow::<Truncy>().unwrap(), Truncy { n: -3 });
        // Plain `/` still works.
        let out: mlua::AnyUserData = lua.load("return t / 2").eval().unwrap();
        assert_eq!(*out.borrow::<Truncy>().unwrap(), Truncy { n: -3 });
    }

    #[test]
    fn float_div_gets_no_idiv_fallback() {
        let lua = Lua::new();
        let tbl = lua.create_table().unwrap();
        bind_type::<Floaty>(&lua, &tbl).unwrap();
        lua.globals()
            .set("x", lua.create_any_userdata(Floaty { v: 7.0 }).unwrap())
            .unwrap();
        assert!(lua.load("return x // 2.0").eval::<mlua::Value>().is_err());
        // `/` unaffected.
        let out: mlua::AnyUserData = lua.load("return x / 2.0").eval().unwrap();
        assert_eq!(*out.borrow::<Floaty>().unwrap(), Floaty { v: 3.5 });
    }
}

#[cfg(not(any(
    feature = "lua55",
    feature = "lua54",
    feature = "lua53",
    feature = "luau"
)))]
#[test]
fn idiv_registration_rejected_before_lua53() {
    let lua = Lua::new();
    let tbl = lua.create_table().unwrap();
    let Err(err) = bind_type::<Ratio>(&lua, &tbl) else {
        panic!("binding an IDiv-declared type must fail before Lua 5.3");
    };
    let haphe_lua::LuaBindError::UnsupportedOperator { op: "idiv" } = err else {
        panic!("expected UnsupportedOperator for idiv, got {err}");
    };
    assert!(err.to_string().contains("floor division"));
}

// ---------------------------------------------------------------------------
// Floor modulo (`%`) — all versions, no gate
// ---------------------------------------------------------------------------

// Explicit Mod: Lua floor semantics (result takes the divisor's sign).
#[derive(Script, Clone, Copy, PartialEq, Debug)]
#[script(traits(Mod, PartialEq))]
struct Wrapped {
    n: i64,
}

impl haphe::ops::Mod for Wrapped {
    type Output = Wrapped;
    fn modulo(self, rhs: Wrapped) -> Wrapped {
        Wrapped {
            n: self.n.rem_euclid(rhs.n),
        }
    }
}

// Rem only: Rust's truncated remainder (dividend's sign).
#[derive(Script, Clone, Copy, PartialEq, Debug)]
#[script(traits(Rem, PartialEq))]
struct Remmy {
    n: i64,
}

impl std::ops::Rem for Remmy {
    type Output = Remmy;
    fn rem(self, rhs: Remmy) -> Remmy {
        Remmy { n: self.n % rhs.n }
    }
}

// Both declared: Mod must win.
#[derive(Script, Clone, Copy, PartialEq, Debug)]
#[script(traits(Mod, Rem, PartialEq))]
struct Both {
    n: i64,
}

impl haphe::ops::Mod for Both {
    type Output = Both;
    fn modulo(self, rhs: Both) -> Both {
        Both {
            n: self.n.rem_euclid(rhs.n),
        }
    }
}

impl std::ops::Rem for Both {
    type Output = Both;
    fn rem(self, rhs: Both) -> Both {
        Both { n: self.n % rhs.n }
    }
}

fn modulo_env<M: haphe::ScriptBind + haphe::ScriptStruct + Clone + Send + Sync + 'static>(
    lua: &Lua,
    a: M,
    b: M,
) {
    let tbl = lua.create_table().unwrap();
    bind_type::<M>(lua, &tbl).unwrap();
    lua.globals()
        .set("a", lua.create_any_userdata(a).unwrap())
        .unwrap();
    lua.globals()
        .set("b", lua.create_any_userdata(b).unwrap())
        .unwrap();
}

#[test]
fn explicit_mod_floors_negative_operands() {
    let lua = Lua::new();
    modulo_env(&lua, Wrapped { n: -7 }, Wrapped { n: 2 });
    let out: mlua::AnyUserData = lua.load("return a % b").eval().unwrap();
    // Lua semantics: -7 % 2 == 1 (result takes the divisor's sign).
    assert_eq!(*out.borrow::<Wrapped>().unwrap(), Wrapped { n: 1 });
}

#[test]
fn rem_only_keeps_truncated_semantics() {
    let lua = Lua::new();
    modulo_env(&lua, Remmy { n: -7 }, Remmy { n: 2 });
    // Rust's `%` truncates: -7 % 2 == -1, NOT Lua's 1. Declare traits(Mod)
    // for genuine Lua `%` behavior on negative operands.
    let out: mlua::AnyUserData = lua.load("return a % b").eval().unwrap();
    assert_eq!(*out.borrow::<Remmy>().unwrap(), Remmy { n: -1 });
}

#[test]
fn mod_wins_over_rem_when_both_declared() {
    let lua = Lua::new();
    modulo_env(&lua, Both { n: -7 }, Both { n: 2 });
    let out: mlua::AnyUserData = lua.load("return a % b").eval().unwrap();
    assert_eq!(*out.borrow::<Both>().unwrap(), Both { n: 1 });
}
