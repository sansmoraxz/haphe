use haphe::script;

#[script]
fn relay<T>(value: T) -> T {
    value
}

haphe::registry! {
    pub static REGISTRY = {
        modules: [
            mod util { functions: [relay<i64, bool>] },
        ],
    };
}

fn main() {}
