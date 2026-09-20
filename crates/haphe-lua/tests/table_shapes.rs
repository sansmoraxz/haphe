//! Empty-table conversion and container-typed member binding: Lua's `{}` is
//! shape-ambiguous (one table type). Core's empty-shape rule accepts either
//! empty shape at the conversion itself, so `{}` converts wherever a
//! container is expected — these tests pin that end to end. Container-typed
//! methods and properties bind like free functions.

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
fn container_typed_methods_bind_and_accept_empty_tables() {
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

// ---------------------------------------------------------------------------
// Nested empties: empty tables inside containers convert recursively
// ---------------------------------------------------------------------------

/// Counts elements per group; empty groups count zero.
#[script]
fn nested_total(groups: Vec<Vec<i64>>) -> i64 {
    groups.iter().map(|g| g.iter().sum::<i64>()).sum()
}

/// Sums every list in a map of lists.
#[script]
fn grouped_total(groups: std::collections::HashMap<String, Vec<i64>>) -> i64 {
    groups.values().map(|g| g.iter().sum::<i64>()).sum()
}

#[test]
fn nested_empty_tables_follow_the_declared_shape() {
    let (lua, module) = env();
    bind_fn::<nested_total>(&lua, &module).unwrap();
    bind_fn::<grouped_total>(&lua, &module).unwrap();

    // An empty table INSIDE a list-typed element converts too.
    let out: i64 = lua
        .load("return m.nested_total({ {}, {1, 2} })")
        .eval()
        .unwrap();
    assert_eq!(out, 3);
    let out: i64 = lua.load("return m.nested_total({})").eval().unwrap();
    assert_eq!(out, 0);
    // ... and inside map VALUES, following the declared value type.
    let out: i64 = lua
        .load("return m.grouped_total({ a = {}, b = {4, 5} })")
        .eval()
        .unwrap();
    assert_eq!(out, 9);
}

// ---------------------------------------------------------------------------
// A constructor and a method may legally share an exposed name (type-table
// vs metatable namespaces); `{}` converts correctly for BOTH signatures —
// core's empty-shape rule needs no per-namespace descriptor pairing
// ---------------------------------------------------------------------------

#[derive(Script, Clone)]
#[script(methods)]
struct Ledger {
    #[script(skip)]
    #[allow(dead_code, reason = "read only through the bound methods")]
    entries: Vec<i64>,
}

#[script]
impl Ledger {
    /// Type-table namespace: `Ledger.reset(seed)` with a LIST parameter.
    #[script(constructor)]
    fn reset(seed: Vec<i64>) -> Self {
        Ledger { entries: seed }
    }

    /// Metatable namespace: `l:reset(weights)` with a MAP parameter.
    #[script(rename = "reset")]
    fn reset_weights(&self, weights: std::collections::HashMap<String, i64>) -> i64 {
        weights.values().sum::<i64>() + self.entries.iter().sum::<i64>()
    }

    fn len(&self) -> i64 {
        self.entries.len() as i64
    }
}

#[test]
fn shared_name_converts_per_signature() {
    let (lua, module) = env();
    let ledger = lua.create_table().unwrap();
    module.set("Ledger", &ledger).unwrap();
    bind_type::<Ledger>(&lua, &ledger).unwrap();

    // `{}` converts through the CONSTRUCTOR's list param, even though a
    // method shares the exposed name with a map param.
    let out: i64 = lua.load("return m.Ledger.reset({}):len()").eval().unwrap();
    assert_eq!(out, 0);
    let out: i64 = lua
        .load("return m.Ledger.reset({7, 8}):len()")
        .eval()
        .unwrap();
    assert_eq!(out, 2);
    // And the method's `{}` stays a map.
    let out: i64 = lua
        .load("local l = m.Ledger.reset({1}) return l:reset({})")
        .eval()
        .unwrap();
    assert_eq!(out, 1);
    let out: i64 = lua
        .load("local l = m.Ledger.reset({1}) return l:reset({ a = 4 })")
        .eval()
        .unwrap();
    assert_eq!(out, 5);
}

// ---------------------------------------------------------------------------
// Generic self types: the SELF instantiation resolves `T`-typed parameters
// during reshaping on the static channels (dyn already merged it)
// ---------------------------------------------------------------------------

#[cfg(feature = "generics")]
mod generic_self {
    use super::*;

    #[derive(Script, Clone)]
    #[script(methods)]
    pub struct Bag<T> {
        #[script(skip)]
        #[allow(dead_code, reason = "read only through the bound methods")]
        pub items: Vec<T>,
    }

    #[script]
    impl<T> Bag<T> {
        #[script(constructor)]
        fn new() -> Self {
            Bag { items: Vec::new() }
        }

        /// Plain method whose parameter is the SELF type's `T`.
        fn put(&mut self, value: T) -> i64 {
            self.items.push(value);
            self.items.len() as i64
        }
    }

    /// `Bag<Vec<i64>>`: the monomorphized wrapper's conversion target is
    /// `Vec<i64>`, so `b:put({})` converts via the empty-shape rule.
    #[test]
    fn self_instantiation_resolves_generic_params_on_static_channels() {
        let lua = Lua::new();
        let table = lua.create_table().unwrap();
        lua.globals().set("BagOfLists", &table).unwrap();
        bind_type::<Bag<Vec<i64>>>(&lua, &table).unwrap();

        let out: i64 = lua
            .load("local b = BagOfLists.new() b:put({1, 2}) return b:put({})")
            .eval()
            .unwrap();
        assert_eq!(out, 2);
    }
}

// ---------------------------------------------------------------------------
// Outbound-only reference composites: readonly fields, returns, getters
// ---------------------------------------------------------------------------

#[derive(Script, Clone)]
#[script(methods)]
struct Catalog {
    #[script(readonly)]
    tags: Vec<&'static str>,
}

#[script]
impl Catalog {
    #[script(constructor)]
    fn new() -> Self {
        Catalog {
            tags: vec!["alpha", "beta"],
        }
    }

    fn all_tags(&self) -> Vec<&'static str> {
        self.tags.clone()
    }

    #[script(getter)]
    fn labels(&self) -> Vec<&'static str> {
        self.tags.clone()
    }
}

fn catalog_env() -> Lua {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    lua.globals().set("Catalog", &table).unwrap();
    bind_type::<Catalog>(&lua, &table).unwrap();
    lua
}

/// A readonly `Vec<&'static str>` field registers through the getter-only
/// channel and reads as a sequence table of native strings.
#[test]
fn outbound_readonly_field_reads_as_string_table() {
    let lua = catalog_env();
    let (len, first, kind): (i64, String, String) = lua
        .load(
            "local c = Catalog.new()
             local t = c.tags
             return #t, t[1], type(t[1])",
        )
        .eval()
        .unwrap();
    assert_eq!((len, first.as_str(), kind.as_str()), (2, "alpha", "string"));
}

/// Writes fail like any other readonly field.
#[test]
fn outbound_readonly_field_rejects_writes() {
    let lua = catalog_env();
    let err = lua
        .load("local c = Catalog.new(); c.tags = { 'x' }")
        .exec()
        .expect_err("no setter registered");
    assert!(!err.to_string().is_empty());
}

/// Methods and getter-only properties returning `Vec<&'static str>` convert
/// outbound at the boundary.
#[test]
fn outbound_reference_returns_convert() {
    let lua = catalog_env();
    let (from_method, from_prop): (String, String) = lua
        .load(
            "local c = Catalog.new()
             return c:all_tags()[2], c.labels[1]",
        )
        .eval()
        .unwrap();
    assert_eq!(
        (from_method.as_str(), from_prop.as_str()),
        ("beta", "alpha")
    );
}
