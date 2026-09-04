use haphe::{Script, script};

#[derive(Script)]
#[script(methods)]
struct Buffer {
    data: Vec<u8>,
}

#[script]
impl Buffer {
    fn merge(&mut self, #[script(clone)] other: &mut Buffer) {
        self.data.extend_from_slice(&other.data);
    }
}

fn main() {}
