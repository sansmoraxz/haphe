use haphe::{Script, script};

#[derive(Script)]
#[script(methods)]
struct Point {
    x: f64,
}

#[script]
impl Point {
    fn get_x(&self) -> f64 {
        self.x
    }
}

#[script]
impl Point {
    fn get_x_again(&self) -> f64 {
        self.x
    }
}

fn main() {}
