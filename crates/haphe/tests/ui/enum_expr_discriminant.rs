use haphe::Script;

const BASE: isize = 4;

#[derive(Script, Clone)]
#[repr(i32)]
enum Coded {
    A = BASE as i32 as isize,
}

fn main() {}
