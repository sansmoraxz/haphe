//! Referencing a generic Script type with a type argument that is not
//! `HapheType` fails at the reference site: the derived `HapheType` impl
//! (which records the concrete arguments for monomorphizing backends)
//! requires every argument to be describable.
use haphe::{HapheType, Script};

#[derive(Script)]
struct Labeled<T> {
    value: T,
}

struct NotScriptable;

static DESC: haphe::TypeDescriptor<'static> =
    <Labeled<NotScriptable> as HapheType>::DESCRIPTOR;

fn main() {}
