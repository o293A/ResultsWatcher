//! Sequential, atomic PNG output: `1.png`, `2.png`, ... never overwriting.
//! Written to a temp file, then renamed, so a folder watcher never sees a partial file.

use std::fs;
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};

/// Next free index = (largest existing `N.png`) + 1.
pub fn next_index(dir: &Path) -> u32 {
    let mut max = 0u32;
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if let Some(stem) = name.strip_suffix(".png") {
                if let Ok(n) = stem.parse::<u32>() {
                    max = max.max(n);
                }
            }
        }
    }
    max + 1
}

/// Encode tightly-packed RGB and publish atomically. Returns the final path.
pub fn save_rgb_atomic(dir: &Path, w: u32, h: u32, rgb: &[u8]) -> io::Result<(u32, PathBuf)> {
    fs::create_dir_all(dir)?;
    let mut n = next_index(dir);
    let mut dst = dir.join(format!("{}.png", n));
    while dst.exists() {
        n += 1; // never overwrite
        dst = dir.join(format!("{}.png", n));
    }
    let tmp = dir.join(format!("{}.png.tmp", n));
    {
        let f = fs::File::create(&tmp)?;
        let mut enc = png::Encoder::new(BufWriter::new(f), w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        enc.set_filter(png::FilterType::Sub);
        let mut wr = enc.write_header().map_err(to_io)?;
        wr.write_image_data(rgb).map_err(to_io)?;
        wr.finish().map_err(to_io)?;
    }
    fs::rename(&tmp, &dst)?;
    Ok((n, dst))
}

fn to_io(e: png::EncodingError) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e.to_string())
}
