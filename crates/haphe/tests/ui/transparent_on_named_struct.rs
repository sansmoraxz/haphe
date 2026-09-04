use haphe::Script;

#[derive(Script)]
#[script(transparent)]
struct Point {
    x: f64,
    y: f64,
}

fn main() {}
