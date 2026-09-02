use haphe::Script;

#[derive(Script)]
enum Color {
    Red,
    Green,
}

haphe::registry! {
    static REGISTRY = {
        structs: [Color],
    };
}

fn main() {}
