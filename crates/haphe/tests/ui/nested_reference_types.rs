use haphe::{Script, script};

#[derive(Script)]
#[script(methods)]
struct Tagger {
    labels: Vec<&'static str>,
}

#[script]
impl Tagger {
    fn label_all(&self, tags: Vec<&str>) -> i64 {
        tags.len() as i64
    }
}

fn main() {}
