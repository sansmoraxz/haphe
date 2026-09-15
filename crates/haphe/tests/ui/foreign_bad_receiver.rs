use haphe::script;

#[script(foreign)]
trait Hooks {
    fn by_value(self, n: i32);
    fn by_mut(&mut self, n: i32);
    fn no_receiver(n: i32);
}

fn main() {}
