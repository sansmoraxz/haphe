use haphe::script;

#[script(foreign)]
trait Hooks {
    fn log(&self, message: &str);
}

fn main() {}
