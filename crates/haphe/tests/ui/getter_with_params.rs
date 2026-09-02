use haphe::{Script, script};

#[derive(Script)]
#[script(methods)]
struct Point {
    x: f64,
}

#[script]
impl Point {
    #[script(getter)]
    fn scaled(&self, factor: f64) -> f64 {
        self.x * factor
    }
}

fn main() {}
