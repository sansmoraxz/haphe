use haphe::script;

#[script]
fn plain(n: i64) -> i64 {
    n
}

haphe::registry! {
    pub static REGISTRY = {
        modules: [
            mod util { functions: [plain<i64>] },
        ],
    };
}

fn main() {}
