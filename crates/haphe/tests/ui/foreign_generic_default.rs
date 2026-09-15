use haphe::script;

#[script(foreign)]
trait Hooks<T = i64> {
    fn get(&self) -> T;
}

fn main() {}
