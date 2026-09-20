haphe::registry! {
    pub static REGISTRY = {
        modules: [
            mod math {
                functions: [
                    double: move |x: i64| -> i64 { x * 2 },
                ],
            },
        ],
    };
}

fn main() {}
