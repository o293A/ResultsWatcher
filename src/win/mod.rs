//! Windows glue. NOT compiled/tested by the author (no Windows toolchain was available):
//! expect small `windows`-crate signature fixes on the first `cargo build --release`.
//! Everything here is external observation: WGC capture, a separate overlay window, Raw Input.
//! No DLL injection, no hook inside Roblox, no memory reading.

mod app;
mod capture;
mod overlay;
mod util;

pub fn run() {
    app::run();
}
