use haphe::Script;

#[derive(Script, Clone)]
#[script(traits(Call(args = i64, output = i64)))]
struct Adder {
    base: i64,
}

fn main() {}
