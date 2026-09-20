use haphe::{Script, script};

#[derive(Script)]
#[script(methods)]
struct Counter {
    value: i64,
}

#[script]
impl Counter {
    #[script(instantiate(i64))]
    fn get(&self) -> i64 {
        self.value
    }
}

fn main() {}
