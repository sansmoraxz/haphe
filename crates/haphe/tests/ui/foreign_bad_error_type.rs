use haphe::script;

pub struct MyError;

#[script(foreign)]
trait Hooks {
    fn get(&self) -> Result<i64, MyError>;
}

fn main() {}
