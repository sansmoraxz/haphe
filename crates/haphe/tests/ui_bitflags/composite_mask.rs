//! `script_bitflags!` rejects composite masks at compile time: WIT-style
//! flags are independent single bits, and `ALL` is a derived combination.
haphe::bitflags::bitflags! {
    #[derive(Clone, Copy)]
    pub struct Perms: u32 {
        const READ = 1;
        const WRITE = 2;
        const ALL = 3;
    }
}

haphe::script_bitflags!(Perms);

fn main() {}
