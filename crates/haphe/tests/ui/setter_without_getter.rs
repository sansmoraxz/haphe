use haphe::{Script, script};

#[derive(Script)]
#[script(methods)]
struct Point {
    x: f64,
}

#[script]
impl Point {
    #[script(setter)]
    fn set_scale(&mut self, value: f64) {
        self.x = value;
    }
}

fn main() {}
