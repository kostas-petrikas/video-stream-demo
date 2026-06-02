# Real-time video streaming demo

### Building dependencies

* Rust toolchain - on MacOS install via Homebrew `brew install rust` 
* ffmpeg - on MacOS install via Homebrew `brew install ffmpeg`

### How to run

* Signaling server: `cargo run --release -p signal_server` (can be accessed via `127.0.0.1:8000` or any IP allocated on the machine)
* GUI: `cargo run --release -p gui`
