//! `#[script(flags)]` marks a bitset enum; structs cannot be flags.
use haphe::Script;

#[derive(Script)]
#[script(flags)]
struct NotFlags {
    bits: u32,
}

fn main() {}
