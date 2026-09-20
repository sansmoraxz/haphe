use haphe::Script;

#[derive(Script)]
enum Color {
    #[script(rename = "dark-blue")]
    DarkBlue,
    #[script(rename = "1st")]
    First,
    Plain,
}

fn main() {}
