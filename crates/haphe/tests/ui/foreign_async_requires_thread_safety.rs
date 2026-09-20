use haphe::script;

#[script(foreign)]
trait Hooks {
    async fn fetch(&self, url: String) -> String;
}

fn main() {}
