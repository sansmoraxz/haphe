use haphe::script;

#[script(foreign)]
trait Hooks<const N: usize> {
    fn ping(&self);
}

fn main() {}
