use haphe::{Script, script};

#[derive(Script)]
#[script(methods)]
struct Point {
    x: f64,
}

#[script]
impl Point {
    #[script(constructor)]
    fn make(n: i32) -> i32 {
        n
    }
}

fn main() {}
