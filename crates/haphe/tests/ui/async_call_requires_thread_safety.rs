use haphe::Script;

#[derive(Script, Clone)]
#[script(traits(AsyncCall(args = (i64,), output = i64)))]
struct Doubler {
    factor: i64,
}

impl haphe::ops::AsyncCall<(i64,)> for Doubler {
    type Output = i64;
    async fn call_async(&self, (n,): (i64,)) -> i64 {
        self.factor * n
    }
}

fn main() {}
