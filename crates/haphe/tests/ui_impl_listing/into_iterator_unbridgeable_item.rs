use haphe::Script;

struct Opaque;

#[derive(Script)]
#[script(traits(IntoIterator(item = Opaque)))]
struct Bag {
    #[script(skip)]
    items: Vec<Opaque>,
}

impl IntoIterator for Bag {
    type Item = Opaque;
    type IntoIter = std::vec::IntoIter<Opaque>;
    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}

fn main() {}
