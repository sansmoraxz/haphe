use haphe::script;

#[script(foreign)]
trait Hooks {
    fn ping(&self) -> i64 {
        0
    }
}

fn main() {}
