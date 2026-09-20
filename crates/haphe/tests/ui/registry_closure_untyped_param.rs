haphe::registry! {
    pub static REGISTRY = {
        modules: [
            mod math {
                functions: [
                    double: |x| -> i64 { x * 2 },
                ],
            },
        ],
    };
}

fn main() {}
