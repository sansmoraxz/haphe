use haphe::Script;

#[derive(Script)]
struct Point {
    #[script(readonyl)]
    x: f64,
}

fn main() {}
