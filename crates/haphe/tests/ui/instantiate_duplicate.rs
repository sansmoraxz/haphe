use haphe::script;

#[script(instantiate(i64), instantiate(String), instantiate(i64))]
fn echo<T>(value: T) -> T {
    value
}

fn main() {}
