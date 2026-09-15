use haphe::script;

#[script(foreign)]
trait Hooks {
    type Output;
    const LIMIT: usize;
    fn get(&self) -> i64;
}

fn main() {}
