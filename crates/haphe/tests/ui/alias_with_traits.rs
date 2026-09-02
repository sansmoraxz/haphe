use haphe::Script;

#[derive(Script, Clone)]
#[script(transparent, traits(Clone), thread_safety = send_sync, methods)]
struct Meters(f64);

fn main() {}
