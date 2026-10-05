//! Pure CPU rendering helpers for the overlay: premultiplied-BGRA canvas, anti-aliased rounded
//! rectangles, box-filter thumbnail, text-mask compositing. Text glyphs themselves are rasterised
//! by GDI in the Windows layer and handed over as an 8-bit coverage mask.

/// Premultiplied BGRA canvas (what `UpdateLayeredWindow` + `AC_SRC_ALPHA` expects).
pub struct Canvas {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

/// Anti-aliased coverage (0..1) of a rounded rectangle at pixel (x, y).
pub fn rounded_cov(x: usize, y: usize, w: usize, h: usize, r: f32) -> f32 {
    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
    let (hw, hh) = (w as f32 / 2.0, h as f32 / 2.0);
    let (qx, qy) = ((px - hw).abs() - (hw - r), (py - hh).abs() - (hh - r));
    let d = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r;
    (0.5 - d).clamp(0.0, 1.0)
}

impl Canvas {
    pub fn new(w: usize, h: usize) -> Canvas {
        Canvas { w, h, px: vec![0; w * h * 4] }
    }

    /// Fill the whole canvas with a rounded rectangle of colour `rgb` and opacity `alpha` (0..1).
    pub fn fill_rounded(&mut self, radius: f32, rgb: [u8; 3], alpha: f32) {
        for y in 0..self.h {
            for x in 0..self.w {
                let a = rounded_cov(x, y, self.w, self.h, radius) * alpha;
                let i = (y * self.w + x) * 4;
                self.px[i] = (rgb[2] as f32 * a) as u8;
                self.px[i + 1] = (rgb[1] as f32 * a) as u8;
                self.px[i + 2] = (rgb[0] as f32 * a) as u8;
                self.px[i + 3] = (a * 255.0) as u8;
            }
        }
    }

    /// Composite `color` (RGB) through an 8-bit coverage mask the size of the canvas.
    pub fn composite_mask(&mut self, mask: &[u8], color: [u8; 3]) {
        for i in 0..self.w * self.h {
            let c = mask[i] as f32 / 255.0;
            if c <= 0.0 {
                continue;
            }
            let o = i * 4;
            let inv = 1.0 - c;
            self.px[o] = (color[2] as f32 * c + self.px[o] as f32 * inv) as u8;
            self.px[o + 1] = (color[1] as f32 * c + self.px[o + 1] as f32 * inv) as u8;
            self.px[o + 2] = (color[0] as f32 * c + self.px[o + 2] as f32 * inv) as u8;
            self.px[o + 3] = (255.0 * c + self.px[o + 3] as f32 * inv) as u8;
        }
    }

    /// Draw an opaque BGRA thumbnail with rounded corners and a thin translucent white border.
    pub fn blit_thumb(&mut self, x0: usize, y0: usize, th: &Thumb, radius: f32, border_px: f32) {
        for y in 0..th.h {
            for x in 0..th.w {
                let cov = rounded_cov(x, y, th.w, th.h, radius);
                if cov <= 0.0 {
                    continue;
                }
                // Border = band between the outer and the inset rounded rect.
                let inner = if x as f32 >= border_px
                    && y as f32 >= border_px
                    && (x as f32) < th.w as f32 - border_px
                    && (y as f32) < th.h as f32 - border_px
                {
                    rounded_inset(x, y, th.w, th.h, radius, border_px)
                } else {
                    0.0
                };
                let i = (y * th.w + x) * 4;
                let (mut b, mut g, mut r) = (th.bgra[i] as f32, th.bgra[i + 1] as f32, th.bgra[i + 2] as f32);
                let edge = (1.0 - inner) * 0.55; // white at 55 % on the border band
                b = b * (1.0 - edge) + 255.0 * edge;
                g = g * (1.0 - edge) + 255.0 * edge;
                r = r * (1.0 - edge) + 255.0 * edge;
                let (cx, cy) = (x0 + x, y0 + y);
                if cx >= self.w || cy >= self.h {
                    continue;
                }
                let o = (cy * self.w + cx) * 4;
                let inv = 1.0 - cov;
                self.px[o] = (b * cov + self.px[o] as f32 * inv) as u8;
                self.px[o + 1] = (g * cov + self.px[o + 1] as f32 * inv) as u8;
                self.px[o + 2] = (r * cov + self.px[o + 2] as f32 * inv) as u8;
                self.px[o + 3] = (255.0 * cov + self.px[o + 3] as f32 * inv) as u8;
            }
        }
    }
}

fn rounded_inset(x: usize, y: usize, w: usize, h: usize, r: f32, b: f32) -> f32 {
    let (iw, ih) = (w as f32 - 2.0 * b, h as f32 - 2.0 * b);
    let (px, py) = (x as f32 + 0.5 - b, y as f32 + 0.5 - b);
    let (hw, hh) = (iw / 2.0, ih / 2.0);
    let rr = (r - b).max(0.0);
    let (qx, qy) = ((px - hw).abs() - (hw - rr), (py - hh).abs() - (hh - rr));
    let d = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - rr;
    (0.5 - d).clamp(0.0, 1.0)
}

/// Small opaque BGRA image.
pub struct Thumb {
    pub w: usize,
    pub h: usize,
    pub bgra: Vec<u8>,
}

/// Area-average (box) downscale of a BGRA image; output alpha is 255.
pub fn box_downscale_bgra(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize) -> Thumb {
    let mut out = vec![255u8; dw * dh * 4];
    for dy in 0..dh {
        let y0 = dy * sh / dh;
        let y1 = (((dy + 1) * sh) / dh).max(y0 + 1).min(sh);
        for dx in 0..dw {
            let x0 = dx * sw / dw;
            let x1 = (((dx + 1) * sw) / dw).max(x0 + 1).min(sw);
            let (mut b, mut g, mut r, mut n) = (0u32, 0u32, 0u32, 0u32);
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y * sw + x) * 4;
                    b += src[i] as u32;
                    g += src[i + 1] as u32;
                    r += src[i + 2] as u32;
                    n += 1;
                }
            }
            let o = (dy * dw + dx) * 4;
            out[o] = (b / n) as u8;
            out[o + 1] = (g / n) as u8;
            out[o + 2] = (r / n) as u8;
        }
    }
    Thumb { w: dw, h: dh, bgra: out }
}
