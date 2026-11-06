//! Compresses a file with Brotli.
//!
//! `cargo run --release --example compress -- QUALITY LGWIN FILE OUT`

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let usage = "usage: compress QUALITY LGWIN FILE OUT";
    let mut args = std::env::args_os().skip(1);
    let quality: u32 = args.next().ok_or(usage)?.to_str().ok_or(usage)?.parse()?;
    let lgwin: u32 = args.next().ok_or(usage)?.to_str().ok_or(usage)?.parse()?;
    let src = std::fs::read(args.next().ok_or(usage)?)?;
    let out = args.next().ok_or(usage)?;
    let stream = oxbrotli::compress_with(&src, oxbrotli::Options::new(quality).lgwin(lgwin));
    std::fs::write(out, stream)?;
    Ok(())
}
