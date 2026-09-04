use haphe::Script;

struct NotDescribed;

#[derive(Script)]
struct Holder {
    inner: NotDescribed,
}

fn main() {}
