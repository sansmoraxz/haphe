haphe::registry! {
    pub static REGISTRY = {
        modules: [
            mod math {
                functions: [
                    double: |x: i64| -> i64 { x * 2 },
                    double: |x: f64| -> f64 { x * 2.0 },
                ],
            },
            // The same name in ANOTHER module is fine: closures live in
            // generated Rust modules mirroring the registry tree.
            mod audio {
                functions: [
                    double: |x: f64| -> f64 { x * 2.0 },
                ],
            },
        ],
    };
}

fn main() {}
