use haphe::{Script, script};

struct Opaque;

#[derive(Script)]
#[script(methods)]
struct Counter {
    value: i64,
}

#[script]
impl Counter {
    #[script(dyn, instantiate(Opaque))]
    fn echo<T>(&self, value: T) -> T {
        value
    }
}

fn main() {}
