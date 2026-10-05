//! Tiny pixel helpers: an RGB image, a `Sampler` abstraction (so the same detection code
//! runs on a PNG file, on an in-memory crop, or on GPU read-back probes) and colour classes.

use crate::geometry::Rect;

/// Anything that can return the RGB of a pixel, or `None` if that pixel is unavailable.
pub trait Sampler {
    fn get(&self, x: i32, y: i32) -> Option<[u8; 3]>;
}

/// Plain 8-bit RGB image (row-major, tightly packed).
#[derive(Clone)]
pub struct Image {
    pub w: usize,
    pub h: usize,
    pub rgb: Vec<u8>,
}

impl Image {
    pub fn new(w: usize, h: usize) -> Image {
        Image { w, h, rgb: vec![0; w * h * 3] }
    }

    /// Build from BGRA rows (Direct3D / WGC native layout).
    pub fn from_bgra(w: usize, h: usize, stride: usize, bgra: &[u8]) -> Image {
        let mut rgb = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            let row = &bgra[y * stride..y * stride + w * 4];
            for px in row.chunks_exact(4) {
                rgb.extend_from_slice(&[px[2], px[1], px[0]]);
            }
        }
        Image { w, h, rgb }
    }

    pub fn set(&mut self, x: i32, y: i32, p: [u8; 3]) {
        if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h {
            let i = (y as usize * self.w + x as usize) * 3;
            self.rgb[i..i + 3].copy_from_slice(&p);
        }
    }

    /// Copy of a sub-rectangle (must lie inside the image).
    pub fn crop(&self, r: Rect) -> Option<Image> {
        if !r.inside(self.w as i32, self.h as i32) {
            return None;
        }
        let mut out = Image::new(r.w as usize, r.h as usize);
        for y in 0..r.h as usize {
            let src = ((r.y as usize + y) * self.w + r.x as usize) * 3;
            out.rgb[y * out.w * 3..(y + 1) * out.w * 3].copy_from_slice(&self.rgb[src..src + out.w * 3]);
        }
        Some(out)
    }

    /// Alpha-blend `p` with opacity `a` (0..1) over a rectangle (test helper: fake tooltip).
    pub fn darken_rect(&mut self, r: Rect, a: f32) {
        for y in r.y.max(0)..r.bottom().min(self.h as i32) {
            for x in r.x.max(0)..r.right().min(self.w as i32) {
                let i = (y as usize * self.w + x as usize) * 3;
                for c in 0..3 {
                    self.rgb[i + c] = (self.rgb[i + c] as f32 * (1.0 - a)) as u8;
                }
            }
        }
    }

    /// Load a PNG (8-bit RGB/RGBA/gray). Used by tests and `--debug-image` only.
    pub fn from_png_file(path: &std::path::Path) -> Result<Image, String> {
        let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let mut dec = png::Decoder::new(std::io::BufReader::new(f));
        dec.set_transformations(png::Transformations::EXPAND);
        let mut rd = dec.read_info().map_err(|e| e.to_string())?;
        let mut buf = vec![0; rd.output_buffer_size()];
        let info = rd.next_frame(&mut buf).map_err(|e| e.to_string())?;
        let (w, h) = (info.width as usize, info.height as usize);
        let mut img = Image::new(w, h);
        match info.color_type {
            png::ColorType::Rgb => img.rgb.copy_from_slice(&buf[..w * h * 3]),
            png::ColorType::Rgba => {
                for i in 0..w * h {
                    img.rgb[i * 3..i * 3 + 3].copy_from_slice(&buf[i * 4..i * 4 + 3]);
                }
            }
            png::ColorType::Grayscale | png::ColorType::GrayscaleAlpha => {
                let step = if info.color_type == png::ColorType::Grayscale { 1 } else { 2 };
                for i in 0..w * h {
                    let g = buf[i * step];
                    img.rgb[i * 3..i * 3 + 3].copy_from_slice(&[g, g, g]);
                }
            }
            other => return Err(format!("unsupported PNG colour type {:?}", other)),
        }
        Ok(img)
    }
}

impl Sampler for Image {
    fn get(&self, x: i32, y: i32) -> Option<[u8; 3]> {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return None;
        }
        let i = (y as usize * self.w + x as usize) * 3;
        Some([self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]])
    }
}

/// (hue degrees 0..360, saturation 0..1, value 0..1)
pub fn hsv(p: [u8; 3]) -> (f32, f32, f32) {
    let (r, g, b) = (p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0);
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let d = mx - mn;
    let h = if d <= 1e-6 {
        0.0
    } else if mx == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if mx == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let h = if h < 0.0 { h + 360.0 } else { h };
    (h, if mx <= 1e-6 { 0.0 } else { d / mx }, mx)
}

pub fn luma(p: [u8; 3]) -> f32 {
    0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
}

fn is_greenish(h: f32) -> bool {
    (148.0..=178.0).contains(&h)
}
fn is_reddish(h: f32) -> bool {
    h <= 14.0 || h >= 346.0
}

/// Bright turquoise/green line: banner border (VICTORY) and active tab outline/fill.
/// Measured reference (30,214,158): h162 s.86 v.84. Loose enough for a slightly blurred crop.
pub fn is_turq(p: [u8; 3]) -> bool {
    let (h, s, v) = hsv(p);
    is_greenish(h) && s >= 0.45 && v >= 0.38
}
/// Bright red line: banner border (DEFEAT). Measured (214,37,31): h2 s.86 v.84.
pub fn is_red_line(p: [u8; 3]) -> bool {
    let (h, s, v) = hsv(p);
    is_reddish(h) && s >= 0.50 && v >= 0.50
}
/// Border colour test used by the full-frame fallback search (tolerates anti-aliased lines).
pub fn is_border_strict(p: [u8; 3]) -> Option<bool> {
    let mx = p[0].max(p[1]).max(p[2]) as i32;
    let mn = p[0].min(p[1]).min(p[2]) as i32;
    if mx < 128 || mx - mn < 80 {
        return None;
    }
    if is_turq(p) {
        Some(true)
    } else if is_red_line(p) {
        Some(false)
    } else {
        None
    }
}
/// Dim saturated team colour: banner fill (27,79,63)/(80,21,20) and row bands (14,63,46)/(64,16,15).
pub fn is_team_green(p: [u8; 3]) -> bool {
    let (h, s, v) = hsv(p);
    is_greenish(h) && s >= 0.45 && (0.12..=0.45).contains(&v)
}
pub fn is_team_red(p: [u8; 3]) -> bool {
    let (h, s, v) = hsv(p);
    is_reddish(h) && s >= 0.45 && (0.12..=0.45).contains(&v)
}
/// Unselected pill outline: light neutral grey (183,186,188).
pub fn is_gray_line(p: [u8; 3]) -> bool {
    let (_, s, v) = hsv(p);
    s <= 0.18 && (0.35..=0.95).contains(&v)
}
/// Panel body is very dark and translucent over a blurred scene (measured max channel 4..25).
pub fn is_dark(p: [u8; 3]) -> bool {
    p[0].max(p[1]).max(p[2]) <= 96
}
