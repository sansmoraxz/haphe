use haphe::script;

#[script(foreign)]
pub trait Convert {
    fn convert<U>(&self, raw: String) -> U;
}

fn main() {}
