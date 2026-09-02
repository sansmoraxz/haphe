use haphe::{Script, script};

#[derive(Script)]
#[script(methods)]
struct Point {
    x: f64,
}

#[script]
impl Point {
    #[script(constructor, getter)]
    fn new(x: f64) -> Self {
        Point { x }
    }
}

fn main() {}
