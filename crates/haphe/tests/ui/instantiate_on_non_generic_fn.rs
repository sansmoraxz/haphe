use haphe::script;

#[script(instantiate(i64))]
fn double(n: i64) -> i64 {
    n * 2
}

fn main() {}
