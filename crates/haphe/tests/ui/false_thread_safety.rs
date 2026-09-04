use std::cell::Cell;

use haphe::Script;

#[derive(Script)]
#[script(thread_safety = send_sync)]
struct Counter {
    #[script(skip)]
    inner: Cell<i32>,
}

fn main() {}
