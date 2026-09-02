use haphe::{Script, script};

#[derive(Script)]
#[script(methods)]
struct Point {
    x: f64,
}

#[script]
impl Point {
    #[script(getter, error_kind = "ValueError")]
    fn scale(&self) -> f64 {
        self.x
    }
}

fn main() {}
