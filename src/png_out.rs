//! Timestamped, atomic PNG output: `Screenshot_2026-10-06_070359.png` (same naming as the native
//! Windows screenshot tool), never overwriting.
//! Written to a temp file, then renamed, so a folder watcher never sees a partial file.

use std::fs;
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};

/// `Screenshot_YYYY-MM-DD_HHMMSS` from a local date/time.
pub fn screenshot_stem(year: u16, month: u16, day: u16, hour: u16, minute: u16, second: u16) -> String {
    format!("Screenshot_{:04}-{:02}-{:02}_{:02}{:02}{:02}", year, month, day, hour, minute, second)
}

/// Encode tightly-packed RGB and publish atomically as `<stem>.png`. If that name already exists
/// (two captures in the same second), `<stem> (2).png`, `<stem> (3).png`, ... is used, like Windows.
pub fn save_rgb_atomic(dir: &Path, w: u32, h: u32, rgb: &[u8], stem: &str) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let mut n = 1u32;
    let mut dst = dir.join(format!("{}.png", stem));
    while dst.exists() {
        n += 1; // never overwrite
        dst = dir.join(format!("{} ({}).png", stem, n));
    }
    let tmp = dir.join(format!("{}.png.tmp", stem));
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
    Ok(dst)
}

fn to_io(e: png::EncodingError) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e.to_string())
}
