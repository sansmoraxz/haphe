use haphe::script;

struct Opaque;

#[script]
fn relay<T>(value: T) -> T {
    value
}

haphe::registry! {
    pub static REGISTRY = {
        modules: [
            mod util { functions: [relay<Opaque>] },
        ],
    };
}

fn main() {}
