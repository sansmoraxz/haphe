use haphe::script;

#[script]
fn identity<T>(value: T) -> T {
    value
}

fn main() {}
