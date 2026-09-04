use haphe::{Script, script};

#[script(thread_safety = none)]
#[derive(Script)]
struct Point {
    x: f64,
}

fn main() {}
