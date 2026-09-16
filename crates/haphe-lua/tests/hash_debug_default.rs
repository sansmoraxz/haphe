//! Hash / Debug / Default interusability surface: Lua has no hashing or
//! debug-formatting protocol, so `traits(Hash)` and `traits(Debug)` register
//! portable `hash()` / `debug()` methods; `traits(Default)` registers an
//! implicit nullary constructor named `default`.

#![allow(dead_code)]

use haphe::Script;
use haphe_lua::bind_type;
use mlua::Lua;

#[derive(Script, Clone, Debug, Hash, PartialEq, Default)]
#[script(traits(Hash, Debug, Default, PartialEq))]
struct Tag {
    id: i64,
    label: String,
}

fn lua_with_tag(value: Tag) -> Lua {
    let lua = Lua::new();
    let tbl = lua.create_table().unwrap();
    bind_type::<Tag>(&lua, &tbl).unwrap();
    lua.globals().set("Tag", tbl).unwrap();
    lua.globals()
        .set("t", lua.create_any_userdata(value).unwrap())
        .unwrap();
    lua
}

#[test]
fn hash_method_is_stable_and_equal_for_equal_values() {
    let lua = lua_with_tag(Tag {
        id: 7,
        label: "x".into(),
    });
    lua.globals()
        .set(
            "u",
            lua.create_any_userdata(Tag {
                id: 7,
                label: "x".into(),
            })
            .unwrap(),
        )
        .unwrap();
    let (a, b, same): (i64, i64, bool) = lua
        .load("local a, b = t:hash(), u:hash() return a, b, a == b")
        .eval()
        .unwrap();
    assert_eq!(a, b);
    assert!(same, "equal values hash equal through the VM");
}

#[test]
fn debug_method_matches_rust_formatting() {
    let tag = Tag {
        id: 3,
        label: "hi".into(),
    };
    let expected = format!("{tag:?}");
    let lua = lua_with_tag(tag);
    let out: String = lua.load("return t:debug()").eval().unwrap();
    assert_eq!(out, expected);
}

#[test]
fn todebugstring_metamethod_feeds_mlua_pretty_debug() {
    // mlua's alternate Debug formatting of a userdata consults
    // `__todebugstring` first — the mlua-native slot meta_debug fills.
    let tag = Tag {
        id: 9,
        label: "dbg".into(),
    };
    let expected = format!("{tag:?}");
    let lua = lua_with_tag(tag);
    let ud: mlua::AnyUserData = lua.load("return t").eval().unwrap();
    let pretty = format!("{ud:#?}");
    assert!(
        pretty.contains(&expected),
        "pretty debug {pretty:?} should contain {expected:?}"
    );
}

#[test]
fn debug_without_display_still_drives_tostring() {
    // The `__tostring` fallback from Debug (no Display declared) is
    // unchanged by the new `debug()` channel.
    let tag = Tag {
        id: 5,
        label: "s".into(),
    };
    let expected = format!("{tag:?}");
    let lua = lua_with_tag(tag);
    let out: String = lua.load("return tostring(t)").eval().unwrap();
    assert_eq!(out, expected);
}

#[test]
fn default_trait_constructs_from_lua() {
    let lua = lua_with_tag(Tag::default());
    let out: mlua::AnyUserData = lua.load("return Tag.default()").eval().unwrap();
    assert_eq!(*out.borrow::<Tag>().unwrap(), Tag::default());
}

// A user method named `hash` on a Hash-declared type would silently shadow
// the implicit method; registration refuses instead.
#[derive(Script, Clone, Debug, Hash)]
#[script(traits(Hash), methods)]
struct Clashing {
    id: i64,
}

#[haphe::script]
impl Clashing {
    fn hash(&self) -> i64 {
        self.id
    }
}

#[test]
fn user_method_named_hash_is_rejected() {
    let lua = Lua::new();
    let tbl = lua.create_table().unwrap();
    let err = bind_type::<Clashing>(&lua, &tbl).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("`hash`"), "unexpected error: {msg}");
}

// ---------------------------------------------------------------------------
// Declaration stubs
// ---------------------------------------------------------------------------

#[test]
fn stubs_cover_hash_debug_and_default() {
    haphe::registry! {
        static STUB_REGISTRY = {
            structs: [Tag],
            modules: [
                mod ids { types: [Tag] },
            ],
        };
    }
    let output = haphe::generate(&haphe_lua::LuaDeclGenerator::new(), &STUB_REGISTRY)
        .expect("generation succeeds");
    let out = String::from_utf8(output.files[0].content.clone()).unwrap();
    assert!(
        out.contains("function Tag:hash() end"),
        "hash stub missing:\n{out}"
    );
    assert!(out.contains("---@return integer"));
    assert!(
        out.contains("function Tag:debug() end"),
        "debug stub missing:\n{out}"
    );
    assert!(
        out.contains("function ids.Tag.default() end"),
        "default ctor stub missing:\n{out}"
    );
}
