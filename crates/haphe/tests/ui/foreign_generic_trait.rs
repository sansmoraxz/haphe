use haphe::script;

#[script(foreign)]
trait Hooks<T> {
    fn get(&self) -> T;
}

fn main() {}
