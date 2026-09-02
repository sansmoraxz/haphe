use haphe::{Script, script};

#[derive(Script)]
#[script(methods)]
struct Point {
    x: f64,
}

#[script]
impl Point {
    #[script(constructor)]
    fn duplicate(&self) -> Self {
        Point { x: self.x }
    }
}

fn main() {}
