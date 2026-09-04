fn plain(a: i32) -> i32 {
    a
}

haphe::registry! {
    static REGISTRY = {
        modules: [
            mod m {
                functions: [plain],
            },
        ],
    };
}

fn main() {}
