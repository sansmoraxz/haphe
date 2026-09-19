use haphe::Script;

#[derive(Script)]
pub struct Point {
    x: f64,
}

// A transparent newtype must wrap a value-convertible inner; a described
// struct is not one.
#[derive(Script)]
#[script(transparent)]
pub struct Wrapped(Point);

fn main() {}
