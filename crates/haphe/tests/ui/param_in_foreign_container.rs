//! A namespaced container carrying a generic parameter cannot be resolved
//! syntactically — it must not be silently described as the std container.
use haphe::Script;

mod collections {
    #[derive(haphe::Script)]
    pub struct Vec<T> {
        #[script(skip)]
        pub items: std::vec::Vec<T>,
    }
}

#[derive(Script)]
struct Holder<T> {
    items: collections::Vec<T>,
}

fn main() {}
