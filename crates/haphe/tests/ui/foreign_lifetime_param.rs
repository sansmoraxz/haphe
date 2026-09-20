use haphe::script;

#[script(foreign)]
trait Hooks<'a> {
    fn ping(&self);
}

fn main() {}
