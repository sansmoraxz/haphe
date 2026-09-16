//! Iteration and indexed access on userdata types: `traits(IntoIterator)` /
//! `traits(Iterator)` drive `pairs`, Luau `for-in`, and the portable
//! `obj:iter()` method; `traits(Index/IndexMut)` drive `obj[key]` access.

#![allow(dead_code)]

use haphe::Script;
use haphe_lua::bind_type;
use mlua::Lua;

// A sequence: plain items iterate as 1-based (i, item).
#[derive(Script, Clone)]
#[script(traits(IntoIterator(item = i64)))]
struct Sequence {
    #[script(skip)]
    values: Vec<i64>,
}

impl IntoIterator for Sequence {
    type Item = i64;
    type IntoIter = std::vec::IntoIter<i64>;
    fn into_iter(self) -> Self::IntoIter {
        self.values.into_iter()
    }
}

// A map-shaped iterable: declared 2-tuple items iterate as (k, v).
#[derive(Script, Clone)]
#[script(traits(IntoIterator(item = (String, i64))))]
struct Scores {
    #[script(skip)]
    entries: Vec<(String, i64)>,
}

impl IntoIterator for Scores {
    type Item = (String, i64);
    type IntoIter = std::vec::IntoIter<(String, i64)>;
    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

// An iterator type bridged through the blanket IntoIterator impl.
#[derive(Script, Clone)]
#[script(traits(Iterator(item = i64)))]
struct Countdown {
    #[script(skip)]
    remaining: i64,
}

impl Iterator for Countdown {
    type Item = i64;
    fn next(&mut self) -> Option<i64> {
        (self.remaining > 0).then(|| {
            self.remaining -= 1;
            self.remaining + 1
        })
    }
}

// Indexed access, coexisting with a field and a method.
#[derive(Script, Clone)]
#[script(
    traits(Index(index = i64, output = i64), IndexMut(index = i64, output = i64)),
    methods
)]
struct Buffer {
    label: String,
    #[script(skip)]
    data: Vec<i64>,
}

#[haphe::script]
impl Buffer {
    fn total(&self) -> i64 {
        self.data.iter().sum()
    }
}

impl haphe::ops::Index<i64> for Buffer {
    type Output = i64;
    fn index(&self, i: i64) -> &i64 {
        &self.data[usize::try_from(i).expect("negative index")]
    }
}

impl haphe::ops::IndexMut<i64> for Buffer {
    fn index_mut(&mut self, i: i64) -> &mut i64 {
        &mut self.data[usize::try_from(i).expect("negative index")]
    }
}

fn lua_with<
    T: haphe::ScriptBind + haphe::ScriptStruct + Clone + mlua::MaybeSend + mlua::MaybeSync + 'static,
>() -> (Lua, mlua::Table) {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    bind_type::<T>(&lua, &table).unwrap();
    (lua, table)
}

fn set_global<T: 'static + mlua::MaybeSend + mlua::MaybeSync>(lua: &Lua, name: &str, value: T) {
    let ud = lua.create_any_userdata(value).unwrap();
    lua.globals().set(name, ud).unwrap();
}

#[test]
fn iter_method_yields_one_based_pairs() {
    let (lua, _t) = lua_with::<Sequence>();
    set_global(
        &lua,
        "seq",
        Sequence {
            values: vec![10, 20, 30],
        },
    );
    let out: String = lua
        .load(
            r#"
            local parts = {}
            for i, v in seq:iter() do
                parts[#parts + 1] = i .. "=" .. v
            end
            return table.concat(parts, ",")
            "#,
        )
        .eval()
        .unwrap();
    assert_eq!(out, "1=10,2=20,3=30");
}

// ---------------------------------------------------------------------------
// Version-specific direct protocols. The un-gated `iter()` tests in this
// file compile and run under EVERY selector, so the portable path is
// exercised in each configuration alongside these direct-path tests.
// ---------------------------------------------------------------------------

/// `pairs(obj)` consults `__pairs` on 5.2+ and LuaJIT with 5.2 compat.
#[cfg(any(
    feature = "lua55",
    feature = "lua54",
    feature = "lua53",
    feature = "lua52",
    feature = "luajit52"
))]
#[test]
fn pairs_builtin_iterates_userdata() {
    let (lua, _t) = lua_with::<Sequence>();
    set_global(&lua, "seq", Sequence { values: vec![7, 8] });
    let out: String = lua
        .load(
            r#"
            local parts = {}
            for i, v in pairs(seq) do
                parts[#parts + 1] = i .. "=" .. v
            end
            return table.concat(parts, ",")
            "#,
        )
        .eval()
        .unwrap();
    assert_eq!(out, "1=7,2=8");
}

/// Both access paths agree where `__pairs` exists: kv items iterate as
/// (k, v) through `pairs(obj)` exactly as through `obj:iter()`.
#[cfg(any(
    feature = "lua55",
    feature = "lua54",
    feature = "lua53",
    feature = "lua52",
    feature = "luajit52"
))]
#[test]
fn pairs_builtin_iterates_key_value() {
    let (lua, _t) = lua_with::<Scores>();
    set_global(
        &lua,
        "scores",
        Scores {
            entries: vec![("a".into(), 1), ("b".into(), 2)],
        },
    );
    let out: String = lua
        .load(
            r#"
            local direct, portable = {}, {}
            for k, v in pairs(scores) do direct[#direct + 1] = k .. "=" .. v end
            for k, v in scores:iter() do portable[#portable + 1] = k .. "=" .. v end
            return table.concat(direct, ",") .. "|" .. table.concat(portable, ",")
            "#,
        )
        .eval()
        .unwrap();
    assert_eq!(out, "a=1,b=2|a=1,b=2");
}

/// `__ipairs` is the 5.2-era array protocol: always 1-based (i, item),
/// stopping at exhaustion — even on a kv-paired type (a 2-tuple item shows
/// up as its 2-element table under `ipairs`).
#[cfg(any(feature = "lua52", feature = "luajit52"))]
#[test]
fn ipairs_builtin_iterates_one_based() {
    let (lua, _t) = lua_with::<Sequence>();
    set_global(&lua, "seq", Sequence { values: vec![7, 8] });
    let out: String = lua
        .load(
            r#"
            local parts = {}
            for i, v in ipairs(seq) do
                parts[#parts + 1] = i .. "=" .. v
            end
            return table.concat(parts, ",")
            "#,
        )
        .eval()
        .unwrap();
    assert_eq!(out, "1=7,2=8");
}

#[cfg(any(feature = "lua52", feature = "luajit52"))]
#[test]
fn ipairs_on_kv_type_stays_sequential() {
    let (lua, _t) = lua_with::<Scores>();
    set_global(
        &lua,
        "scores",
        Scores {
            entries: vec![("a".into(), 1)],
        },
    );
    let out: String = lua
        .load(
            r#"
            local parts = {}
            for i, v in ipairs(scores) do
                parts[#parts + 1] = i .. "=" .. v[1] .. ":" .. v[2]
            end
            return table.concat(parts, ",")
            "#,
        )
        .eval()
        .unwrap();
    assert_eq!(out, "1=a:1");
}

/// Luau's native `for ... in obj do` goes through `__iter`.
#[cfg(feature = "luau")]
#[test]
fn luau_native_for_in_iterates_userdata() {
    let (lua, _t) = lua_with::<Sequence>();
    set_global(&lua, "seq", Sequence { values: vec![7, 8] });
    let out: String = lua
        .load(
            r#"
            local parts = {}
            for i, v in seq do
                parts[#parts + 1] = i .. "=" .. v
            end
            return table.concat(parts, ",")
            "#,
        )
        .eval()
        .unwrap();
    assert_eq!(out, "1=7,2=8");
}

/// Both access paths agree on Luau: kv items iterate as (k, v) through the
/// native for-in exactly as through the portable `obj:iter()`.
#[cfg(feature = "luau")]
#[test]
fn luau_native_for_in_key_value_matches_iter() {
    let (lua, _t) = lua_with::<Scores>();
    set_global(
        &lua,
        "scores",
        Scores {
            entries: vec![("a".into(), 1), ("b".into(), 2)],
        },
    );
    let out: String = lua
        .load(
            r#"
            local direct, portable = {}, {}
            for k, v in scores do direct[#direct + 1] = k .. "=" .. v end
            for k, v in scores:iter() do portable[#portable + 1] = k .. "=" .. v end
            return table.concat(direct, ",") .. "|" .. table.concat(portable, ",")
            "#,
        )
        .eval()
        .unwrap();
    assert_eq!(out, "a=1,b=2|a=1,b=2");
}

#[test]
fn tuple_items_iterate_as_key_value() {
    let (lua, _t) = lua_with::<Scores>();
    set_global(
        &lua,
        "scores",
        Scores {
            entries: vec![("a".into(), 1), ("b".into(), 2)],
        },
    );
    let out: String = lua
        .load(
            r#"
            local parts = {}
            for k, v in scores:iter() do
                parts[#parts + 1] = k .. "=" .. v
            end
            return table.concat(parts, ",")
            "#,
        )
        .eval()
        .unwrap();
    assert_eq!(out, "a=1,b=2");
}

#[test]
fn iterator_trait_bridges_like_into_iterator() {
    let (lua, _t) = lua_with::<Countdown>();
    set_global(&lua, "count", Countdown { remaining: 3 });
    let out: String = lua
        .load(
            r#"
            local parts = {}
            for _, v in count:iter() do
                parts[#parts + 1] = tostring(v)
            end
            return table.concat(parts, ",")
            "#,
        )
        .eval()
        .unwrap();
    assert_eq!(out, "3,2,1");
}

#[test]
fn empty_collection_iterates_zero_times() {
    let (lua, _t) = lua_with::<Sequence>();
    set_global(&lua, "seq", Sequence { values: vec![] });
    let n: i64 = lua
        .load(
            r#"
            local n = 0
            for _ in seq:iter() do n = n + 1 end
            return n
            "#,
        )
        .eval()
        .unwrap();
    assert_eq!(n, 0);
}

#[test]
fn concurrent_iterations_are_independent() {
    let (lua, _t) = lua_with::<Sequence>();
    set_global(&lua, "seq", Sequence { values: vec![1, 2] });
    let out: String = lua
        .load(
            r#"
            local a = seq:iter()
            local b = seq:iter()
            local _, a1 = a()
            local _, b1 = b()
            local _, a2 = a()
            local _, b2 = b()
            return a1 .. b1 .. a2 .. b2
            "#,
        )
        .eval()
        .unwrap();
    assert_eq!(out, "1122");
}

#[test]
fn length_operator_reports_size_hint() {
    let (lua, _t) = lua_with::<Sequence>();
    set_global(
        &lua,
        "seq",
        Sequence {
            values: vec![5, 6, 7],
        },
    );
    let n: i64 = lua.load("return #seq").eval().unwrap();
    assert_eq!(n, 3);
}

#[test]
fn index_reads_and_writes() {
    let (lua, _t) = lua_with::<Buffer>();
    set_global(
        &lua,
        "buf",
        Buffer {
            label: "b".into(),
            data: vec![7, 8],
        },
    );
    let v: i64 = lua.load("return buf[1]").eval().unwrap();
    assert_eq!(v, 8);
    lua.load("buf[0] = 99").exec().unwrap();
    let v: i64 = lua.load("return buf[0]").eval().unwrap();
    assert_eq!(v, 99);
}

#[test]
fn fields_and_methods_win_over_custom_index() {
    let (lua, _t) = lua_with::<Buffer>();
    set_global(
        &lua,
        "buf",
        Buffer {
            label: "mine".into(),
            data: vec![1, 2, 3],
        },
    );
    let label: String = lua.load("return buf.label").eval().unwrap();
    assert_eq!(label, "mine");
    let total: i64 = lua.load("return buf:total()").eval().unwrap();
    assert_eq!(total, 6);
    // Misses still reach the custom metamethod.
    let v: i64 = lua.load("return buf[2]").eval().unwrap();
    assert_eq!(v, 3);
}

#[test]
fn index_key_conversion_failure_errors() {
    let (lua, _t) = lua_with::<Buffer>();
    set_global(
        &lua,
        "buf",
        Buffer {
            label: "b".into(),
            data: vec![1],
        },
    );
    // A boolean key cannot convert to the declared i64 index type. (A string
    // key would be swallowed by the field/method lookup path instead.)
    let err = lua.load("return buf[true]").eval::<i64>().unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("expected"), "unexpected error: {msg}");
}

// An out-of-bounds Rust panic becomes a Lua error that `pcall` observes
// (mlua resumes the panic if it crosses back into Rust uncaught).
#[test]
fn out_of_bounds_index_errors_in_lua() {
    let (lua, _t) = lua_with::<Buffer>();
    set_global(
        &lua,
        "buf",
        Buffer {
            label: "b".into(),
            data: vec![1],
        },
    );
    let ok: bool = lua
        .load("local ok = pcall(function() return buf[10] end) return ok")
        .eval()
        .unwrap();
    assert!(!ok);
    let ok: bool = lua
        .load("local ok = pcall(function() buf[10] = 1 end) return ok")
        .eval()
        .unwrap();
    assert!(!ok);
}

// A user method named `iter` on an iterable type is rejected at bind time.
#[derive(Script, Clone)]
#[script(traits(IntoIterator(item = i64)), methods)]
struct Clashing {
    #[script(skip)]
    values: Vec<i64>,
}

impl IntoIterator for Clashing {
    type Item = i64;
    type IntoIter = std::vec::IntoIter<i64>;
    fn into_iter(self) -> Self::IntoIter {
        self.values.into_iter()
    }
}

#[haphe::script]
impl Clashing {
    fn iter(&self) -> i64 {
        0
    }
}

#[test]
fn user_iter_method_on_iterable_type_is_rejected() {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    let err = bind_type::<Clashing>(&lua, &table).unwrap_err();
    assert!(matches!(
        err,
        haphe_lua::LuaBindError::ReservedMethod { name: "iter" }
    ));
}

// Documented 5.1/plain-LuaJIT gap: no `__pairs`, so `pairs(obj)` raises,
// while the portable `obj:iter()` still works in the same configuration.
#[cfg(any(feature = "lua51", all(feature = "luajit", not(feature = "luajit52"))))]
#[test]
fn pairs_gap_on_lua51_iter_still_works() {
    let (lua, _t) = lua_with::<Sequence>();
    set_global(&lua, "seq", Sequence { values: vec![4, 5] });
    assert!(lua.load("for _ in pairs(seq) do end").exec().is_err());
    let out: String = lua
        .load(
            r#"
            local parts = {}
            for i, v in seq:iter() do
                parts[#parts + 1] = i .. "=" .. v
            end
            return table.concat(parts, ",")
            "#,
        )
        .eval()
        .unwrap();
    assert_eq!(out, "1=4,2=5");
}
