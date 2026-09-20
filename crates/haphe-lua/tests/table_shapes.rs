//! Empty-table reshaping and container-typed member binding: Lua's `{}` is
//! shape-ambiguous (one table type), so the binder re-shapes empty arguments
//! by the DECLARED parameter type; container-typed methods and properties
//! bind like free functions.

#![allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    reason = "fixture shapes are dictated by the bridge surface under test: `#[script]` functions receive OWNED values (the boundary contract), and methods keep their declared receivers"
)]

use haphe::{Script, script};
use haphe_lua::{bind_fn, bind_type};
use mlua::Lua;

/// Sums a list.
#[script]
fn total(values: Vec<i64>) -> i64 {
    values.iter().sum()
}

/// Counts a map's entries.
#[script]
fn entry_count(map: std::collections::HashMap<String, i64>) -> i64 {
    map.len() as i64
}

#[derive(Script, Clone)]
#[script(methods)]
struct Basket {
    #[script(skip)]
    #[allow(dead_code, reason = "read only through the bound methods and property")]
    items: Vec<i64>,
}

#[script]
impl Basket {
    #[script(constructor)]
    fn new() -> Self {
        Basket { items: vec![1, 2] }
    }

    fn add_all(&mut self, more: Vec<i64>) -> i64 {
        self.items.extend(more);
        self.items.iter().sum()
    }

    fn snapshot(&self) -> Vec<i64> {
        self.items.clone()
    }

    #[script(getter)]
    fn labels(&self) -> Vec<String> {
        self.items.iter().map(ToString::to_string).collect()
    }
}

fn env() -> (Lua, mlua::Table) {
    let lua = Lua::new();
    let module = lua.create_table().unwrap();
    lua.globals().set("m", &module).unwrap();
    bind_fn::<total>(&lua, &module).unwrap();
    bind_fn::<entry_count>(&lua, &module).unwrap();
    let basket = lua.create_table().unwrap();
    module.set("Basket", &basket).unwrap();
    bind_type::<Basket>(&lua, &basket).unwrap();
    (lua, module)
}

/// `{}` to a list-typed parameter is the empty list, not a rejected map.
#[test]
fn empty_table_crosses_as_empty_list() {
    let (lua, _) = env();
    let out: i64 = lua.load("return m.total({})").eval().unwrap();
    assert_eq!(out, 0);
    let out: i64 = lua.load("return m.total({4, 5})").eval().unwrap();
    assert_eq!(out, 9);
}

/// `{}` to a map-typed parameter still crosses as the empty map.
#[test]
fn empty_table_crosses_as_empty_map() {
    let (lua, _) = env();
    let out: i64 = lua.load("return m.entry_count({})").eval().unwrap();
    assert_eq!(out, 0);
    let out: i64 = lua
        .load("return m.entry_count({ a = 1, b = 2 })")
        .eval()
        .unwrap();
    assert_eq!(out, 2);
}

/// Container-typed METHODS bind (free-fn gate parity), including the
/// empty-table argument, and a `Vec` return crosses as a sequence table.
#[test]
fn container_typed_methods_bind_and_reshape() {
    let (lua, _) = env();
    let out: i64 = lua
        .load("local b = m.Basket.new() return b:add_all({3, 4})")
        .eval()
        .unwrap();
    assert_eq!(out, 10);
    let out: i64 = lua
        .load("local b = m.Basket.new() return b:add_all({})")
        .eval()
        .unwrap();
    assert_eq!(out, 3);
    let (len, first): (i64, i64) = lua
        .load("local s = m.Basket.new():snapshot() return #s, s[1]")
        .eval()
        .unwrap();
    assert_eq!((len, first), (2, 1));
}

/// Container-typed computed properties bind and cross as tables.
#[test]
fn container_typed_property_binds() {
    let (lua, _) = env();
    let label: String = lua.load("return m.Basket.new().labels[2]").eval().unwrap();
    assert_eq!(label, "2");
}

/// Namespace regressions: a dyn ASYNC associated fn shares the type-table
/// namespace with constructors, and a mangled static-generic monomorph is
/// checked against async dyn method names — both collisions are loud, never
/// silent overwrites.
#[cfg(all(feature = "generics", feature = "async", not(feature = "send")))]
mod namespace_gaps {
    use haphe::{Script, script};
    use haphe_lua::bind_type;
    use mlua::Lua;

    #[derive(Script, Clone)]
    #[script(thread_safety = send_sync, methods)]
    struct CtorClash {
        n: i64,
    }

    #[script]
    impl CtorClash {
        #[script(constructor)]
        fn new() -> Self {
            CtorClash { n: 0 }
        }

        /// Renamed onto the constructor's name: same type-table slot.
        #[script(rename = "new", dyn, instantiate(i64))]
        async fn make_later<T>(v: T) -> T {
            v
        }
    }

    #[test]
    fn dyn_async_associated_shares_the_ctor_namespace() {
        let lua = Lua::new();
        let tbl = lua.create_table().unwrap();
        let err = bind_type::<CtorClash>(&lua, &tbl).unwrap_err();
        assert!(
            matches!(
                &err,
                haphe_lua::LuaBindError::DuplicateConstructor { name } if *name == "new"
            ),
            "expected DuplicateConstructor, got {err}"
        );
    }

    #[derive(Script, Clone)]
    #[script(thread_safety = send_sync, methods)]
    struct MangleClash {
        n: i64,
    }

    #[script]
    impl MangleClash {
        /// Static monomorph mangling to `pick__i64`.
        #[script(instantiate(i64))]
        fn pick<T>(&self, v: T) -> T {
            v
        }

        /// Renamed onto the mangled monomorph's slot.
        #[script(rename = "pick__i64", dyn, instantiate(String))]
        async fn pick_later<T>(&self, v: T) -> T {
            v
        }
    }

    #[test]
    fn mangled_monomorph_checked_against_async_dyn_names() {
        let lua = Lua::new();
        let tbl = lua.create_table().unwrap();
        let err = bind_type::<MangleClash>(&lua, &tbl).unwrap_err();
        assert!(
            matches!(
                &err,
                haphe_lua::LuaBindError::DuplicateMethod { name } if name == "pick__i64"
            ),
            "expected DuplicateMethod, got {err}"
        );
    }
}
