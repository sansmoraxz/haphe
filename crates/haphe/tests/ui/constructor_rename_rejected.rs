use haphe::{Script, script};

#[derive(Script)]
#[script(methods)]
struct Point {
    x: f64,
}

#[script]
impl Point {
    #[script(constructor, rename = "create")]
    fn new(x: f64) -> Self {
        Point { x }
    }
}

fn main() {}
