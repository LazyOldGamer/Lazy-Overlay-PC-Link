# Lazy Overlay PC Link

Windows and Linux utility that lets Lazy Overlay start/stop programs, pick or watch files, and show notifications, limited to what you allow in its window.

## Download

`bin/`: `lazy-overlay-link-setup.exe` (Windows installer), `lazy-overlay-link.exe` (Windows), `lazy-overlay-link-linux-x64` (Linux). Checksums in `bin/SHA256SUMS`.

## Build

Needs Rust 1.99+ ([rustup](https://rustup.rs)).

- **Windows:** Visual Studio Build Tools (C++ workload) and the Windows SDK.
- **Linux:** a C toolchain; OpenSSL 3 or 4 at run time.

```sh
cargo build --release   # target/release/lazy-overlay-link(.exe)
cargo test
```

Windows installer (optional, [Inno Setup](https://jrsoftware.org/isinfo.php) 7):

```sh
iscc /DVersion=1.0.1 installer/pclink.iss
```
