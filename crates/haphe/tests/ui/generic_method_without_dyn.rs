use haphe::{Script, script};

#[derive(Script)]
#[script(methods)]
struct Counter {
    value: i64,
}

#[script]
impl Counter {
    fn echo<T>(&self, value: T) -> T {
        value
    }
}

fn main() {}
