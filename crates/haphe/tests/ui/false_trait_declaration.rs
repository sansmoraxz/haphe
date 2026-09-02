use haphe::Script;

#[derive(Script)]
#[script(traits(Display))]
struct Silent {
    x: f64,
}

fn main() {}
