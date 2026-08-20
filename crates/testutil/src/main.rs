//! Writes a crafted ZIM archive. Test fixture generator, not a ZIM writer.

use std::io::Write;

fn main() {
    let mut args = std::env::args().skip(1);
    let kind = args.next().unwrap_or_else(|| "sample".into());
    let path = args.next();

    let bytes = match kind.as_str() {
        "sample" => testutil::sample().build(),
        "zstd" => testutil::sample().compression(testutil::Compression::Zstd).build(),
        "xz" => testutil::sample().compression(testutil::Compression::Xz).build(),
        "legacy" => testutil::Builder::new()
            .version(5, 0)
            .mimes(["text/html"])
            .content("index.html", "Main Page", 0, b"<html>legacy</html>")
            .content_in(b'I', "logo.png", "Logo", 0, b"png")
            .build(),
        other => {
            eprintln!("zim-craft: unknown archive kind {other}");
            std::process::exit(2);
        }
    };

    match path {
        Some(p) => std::fs::write(&p, &bytes).expect("write archive"),
        None => std::io::stdout().write_all(&bytes).expect("write archive"),
    }
}
