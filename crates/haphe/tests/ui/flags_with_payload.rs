//! `#[script(flags)]` enums are bitsets: variants cannot carry payloads.
use haphe::Script;

#[derive(Script)]
#[script(flags)]
enum Perm {
    Read,
    Write,
    Custom(u32),
}

fn main() {}
