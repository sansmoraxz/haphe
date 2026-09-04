use haphe::Script;

#[derive(Script)]
#[script(traits(Debug, Debug))]
#[derive(Debug)]
struct Point {
    x: f64,
}

fn main() {}
