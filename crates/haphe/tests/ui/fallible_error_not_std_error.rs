use haphe::{Script, script};

struct Opaque;

impl std::fmt::Display for Opaque {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("opaque")
    }
}

#[derive(Script)]
#[script(methods)]
struct Counter {
    value: i64,
}

#[script]
impl Counter {
    fn checked(&self) -> Result<i64, Opaque> {
        Err(Opaque)
    }
}

fn main() {}
