use haphe::Script;

#[derive(Script)]
struct Packet {
    #[script(bytes)]
    payload: Vec<i32>,
}

fn main() {}
