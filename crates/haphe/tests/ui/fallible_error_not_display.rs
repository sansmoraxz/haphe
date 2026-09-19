use haphe::{Script, script};

struct Opaque;

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
