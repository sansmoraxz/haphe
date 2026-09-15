//! Registering a generic instantiation whose type argument is not
//! `HapheType` fails inside `registry!`: the instantiation record needs a
//! descriptor for every concrete argument.
use haphe::Script;

#[derive(Script)]
struct Labeled<T> {
    value: T,
}

struct NotScriptable;

haphe::registry! {
    pub static REGISTRY = {
        structs: [Labeled<NotScriptable>],
    };
}

fn main() {}
