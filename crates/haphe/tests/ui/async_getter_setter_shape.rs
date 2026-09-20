use haphe::{Script, script};

#[derive(Script, Clone)]
#[script(thread_safety = send_sync, methods)]
struct Probe {
    #[script(skip)]
    id: i64,
}

#[script]
impl Probe {
    // Async getters keep the shape rules: &self only, no params.
    #[script(getter)]
    async fn id(&mut self) -> i64 {
        self.id
    }

    #[script(getter)]
    async fn labeled(&self, prefix: String) -> String {
        format!("{prefix}{}", self.id)
    }

    // Async setters: &mut self, exactly one param, no return.
    #[script(setter)]
    async fn set_id(&self, value: i64) -> i64 {
        value
    }
}

fn main() {}
