use haphe::Script;

#[derive(Script)]
struct Hooks {
    on_name: fn(&str) -> i64,
}

fn main() {}
