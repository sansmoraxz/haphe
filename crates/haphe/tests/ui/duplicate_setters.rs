use haphe::{Script, script};

#[derive(Script)]
#[script(methods)]
struct Point {
    x: f64,
}

#[script]
impl Point {
    #[script(getter)]
    fn scale(&self) -> f64 {
        self.x
    }

    #[script(setter)]
    fn set_scale(&mut self, value: f64) {
        self.x = value;
    }

    #[script(setter = "scale")]
    fn set_scale_str(&mut self, value: String) {
        self.x = value.len() as f64;
    }
}

fn main() {}
