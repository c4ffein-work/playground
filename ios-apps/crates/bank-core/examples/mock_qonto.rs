//! The mock of the Qonto API (`tests/mock/mod.rs`), as a program: for the tests of the Swift package.
//!   cargo run -p bank-core --example mock_qonto -- <file to write the URL to>
//! Serves the sample data on a free port of 127.0.0.1 until it is killed. The credentials it accepts
//! are `mock::SLUG` and `mock::SECRET`.

#[path = "../tests/mock/mod.rs"]
mod mock;

fn main() {
    let url_file = std::env::args()
        .nth(1)
        .expect("usage: mock_qonto <file to write the URL to>");
    let server = mock::serve(mock::State::sample());
    // Written whole, then renamed: whoever waits for the file never reads half a URL.
    let partial = format!("{url_file}.part");
    std::fs::write(&partial, &server.url).unwrap();
    std::fs::rename(&partial, &url_file).unwrap();
    println!("mock Qonto API on {} ({}:{})", server.url, mock::SLUG, mock::SECRET);
    loop {
        std::thread::park();
    }
}
