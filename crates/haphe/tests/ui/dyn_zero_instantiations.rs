use haphe::script;

// Multi-parameter generics get no default candidate set (no Cartesian
// default) — explicit `instantiate(...)` is required.
#[script(dyn)]
fn pair<T, U>(a: T, b: U) -> T {
    let _ = b;
    a
}

fn main() {}
