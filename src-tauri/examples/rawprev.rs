//! Dump the embedded JPEG preview out of every RAW file in a folder.
//!
//! Use this when adding support for a camera or format that misbehaves: it
//! reports the preview's size and dimensions and writes it to `_previews/` so
//! you can confirm it is the actual photograph rather than a thumbnail or a
//! false positive found in sensor data.
//!
//!   cargo run --release --example rawprev -- ../demo_raw
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let dir = PathBuf::from(std::env::args().nth(1).expect("folder"));
    let out = dir.join("_previews");
    std::fs::create_dir_all(&out)?;
    for e in std::fs::read_dir(&dir)? {
        let p = e?.path();
        if !culling_lib::scan::is_raw(&p) {
            continue;
        }
        let t = std::time::Instant::now();
        match culling_lib::raw::extract_preview(&p) {
            Ok(jpeg) => {
                let mut d = jpeg_decoder::Decoder::new(std::io::Cursor::new(&jpeg));
                let dims = d.read_info().ok().and_then(|_| d.info()).map(|i| (i.width, i.height));
                println!(
                    "{:<45} preview {:>7} KB  {:?}  in {} ms",
                    p.file_name().unwrap().to_string_lossy(),
                    jpeg.len() / 1024,
                    dims,
                    t.elapsed().as_millis()
                );
                let name = p.file_stem().unwrap().to_string_lossy().to_string() + ".jpg";
                std::fs::write(out.join(name), &jpeg)?;
            }
            Err(err) => println!("{:<45} FAILED: {err}", p.file_name().unwrap().to_string_lossy()),
        }
    }
    println!("written to {}", out.display());
    Ok(())
}
