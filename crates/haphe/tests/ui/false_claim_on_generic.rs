//! A false thread-safety claim on a generic type fails at the instantiation
//! that exposes it (via the where-bound on the descriptor impl).
use haphe::{Script, ScriptStruct};

#[derive(Script)]
#[script(thread_safety = send_sync)]
struct Holder<T> {
    value: T,
}

static DESC: haphe::StructDescriptor<'static> =
    <Holder<std::cell::Cell<i32>> as ScriptStruct>::DESCRIPTOR;

fn main() {}
