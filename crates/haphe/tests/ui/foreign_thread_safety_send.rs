use haphe::script;

#[script(foreign, thread_safety = send_sync)]
trait Hooks {
    fn ping(&self);
}

fn main() {}
