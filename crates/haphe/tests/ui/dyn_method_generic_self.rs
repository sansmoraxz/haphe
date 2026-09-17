use haphe::{Script, script};

#[derive(Script)]
#[script(methods)]
struct Wrapper<T: Clone + 'static> {
    value: T,
}

#[script]
impl<T: Clone> Wrapper<T> {
    #[script(dyn)]
    fn echo<U>(&self, value: U) -> U {
        value
    }
}

fn main() {}
