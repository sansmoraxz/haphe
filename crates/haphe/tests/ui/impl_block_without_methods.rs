use haphe::{Script, script};

#[derive(Script)]
struct Point {
    x: f64,
}

#[script]
impl Point {
    fn get_x(&self) -> f64 {
        self.x
    }
}

fn main() {}
