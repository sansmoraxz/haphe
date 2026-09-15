use haphe::script;

#[script(instantiate(i64, String))]
fn echo<T>(value: T) -> T {
    value
}

fn main() {}
