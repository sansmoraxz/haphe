use haphe::{Script, script};

#[derive(Script)]
#[script(methods)]
struct Counter {
    value: i64,
}

#[script]
impl Counter {
    #[script(constructor)]
    fn of<T: Into<i64>>(value: T) -> Self {
        Counter {
            value: value.into(),
        }
    }
}

fn main() {}
