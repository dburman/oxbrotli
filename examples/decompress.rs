//! Decompresses a Brotli file to standard output.
//!
//! `cargo run --release --example decompress -- FILE.br > FILE`

use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: decompress FILE.br > FILE")?;
    let src = std::fs::read(path)?;
    let out = oxbrotli::decompress(&src)?;
    std::io::stdout().lock().write_all(&out)?;
    Ok(())
}
