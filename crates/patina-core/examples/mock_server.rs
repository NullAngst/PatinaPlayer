//! A stand-in Navidrome on localhost, for trying the apps without a real
//! server: `cargo run -p patina-core --example mock_server`. Log in as
//! `demo` with password `demo`. Needs `ffmpeg`. The data is made up and
//! lives in memory only.

#[path = "../tests/common/mod.rs"]
mod common;

fn main() {
    let srv = common::Subsonic::start(common::Flavor::Navidrome, "demo", "demo", None);
    println!(
        "Stand-in Navidrome at {}  (user demo, password demo)",
        srv.url
    );
    loop {
        std::thread::park();
    }
}
