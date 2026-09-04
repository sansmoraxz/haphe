use haphe::{Script, script};

#[derive(Script)]
#[script(methods)]
struct Fetcher {
    url: String,
}

#[script]
impl Fetcher {
    async fn fetch(&self) -> String {
        self.url.clone()
    }
}

fn main() {}
