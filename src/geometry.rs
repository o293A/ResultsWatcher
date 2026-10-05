//! Panel geometry. Every constant below was MEASURED on the reference captures
//! (1920x1080, see tests/fixtures) in "reference units" = pixels of the panel at
//! scale 1.0, origin = top-left pixel of the panel.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Rect { x, y, w, h }
    }
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }
    pub fn inside(&self, w: i32, h: i32) -> bool {
        self.x >= 0 && self.y >= 0 && self.right() <= w && self.bottom() <= h && self.w > 0 && self.h > 0
    }
    pub fn grow(&self, d: i32) -> Rect {
        Rect::new(self.x - d, self.y - d, self.w + 2 * d, self.h + 2 * d)
    }
}

// ---- measured constants (reference units) -------------------------------------------------
/// Panel outer size (measured 719 x 525; panel.png is a resampled 715x526 crop, not exact).
pub const PANEL_W: f64 = 719.0;
pub const PANEL_H: f64 = 525.0;
/// Reference client width. Measured: the panel keeps the SAME pixel size in a 1920x1080
/// client and in a 1920x1009 windowed client, so scale follows the client WIDTH (or is
/// fixed); it does NOT follow the height. See `scale_candidates`.
pub const REF_CLIENT_W: f64 = 1920.0;
/// Horizontal shift of the panel from client centre: +19 px at width 1920 (~ +0.99 %).
pub const X_SHIFT_FRAC: f64 = 19.0 / 1920.0;

pub const BANNER: Rect = Rect { x: 32, y: 143, w: 655, h: 62 };
pub const STATS_PILL: Rect = Rect { x: 384, y: 91, w: 72, h: 30 };
pub const REWARDS_PILL: Rect = Rect { x: 261, y: 90, w: 106, h: 32 };
pub const TITLE_ICON: Rect = Rect { x: 19, y: 22, w: 195, h: 36 };
pub const CLOSE_X: Rect = Rect { x: 657, y: 20, w: 40, h: 40 };
pub const ROW_FIRST_Y: i32 = 255;
pub const ROW_H: i32 = 40;
pub const ROW_X: i32 = 46;

/// Client area inside the captured frame, in frame pixels (the frame of a window capture
/// includes the title bar, so the client origin is generally not (0,0)).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClientBox {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Where the panel sits in the frame for a given scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub scale: f64,
    pub ox: f64,
    pub oy: f64,
}

impl Layout {
    /// Predict the panel origin: centred vertically in the client area, centred
    /// horizontally + 1 % of the client width. No hard-coded absolute position.
    pub fn predicted(c: &ClientBox, scale: f64) -> Layout {
        let pw = PANEL_W * scale;
        let ph = PANEL_H * scale;
        let ox = c.x as f64 + (c.w as f64 - pw) / 2.0 + X_SHIFT_FRAC * c.w as f64;
        let oy = c.y as f64 + (c.h as f64 - ph) / 2.0;
        Layout { scale, ox, oy }
    }
    pub fn from_origin(scale: f64, ox: f64, oy: f64) -> Layout {
        Layout { scale, ox, oy }
    }
    pub fn pt(&self, rx: f64, ry: f64) -> (i32, i32) {
        ((self.ox + rx * self.scale).round() as i32, (self.oy + ry * self.scale).round() as i32)
    }
    /// Reference rectangle -> frame rectangle (at least 1x1).
    pub fn rect(&self, r: Rect) -> Rect {
        let (x0, y0) = self.pt(r.x as f64, r.y as f64);
        let (x1, y1) = self.pt((r.x + r.w) as f64, (r.y + r.h) as f64);
        Rect::new(x0, y0, (x1 - x0).max(1), (y1 - y0).max(1))
    }
    pub fn panel_rect(&self) -> Rect {
        self.rect(Rect::new(0, 0, PANEL_W as i32, PANEL_H as i32))
    }
}

/// Scale hypotheses, most likely first. Evidence: panel pixel size identical at client
/// heights 1080 and 1009 -> width-based or fixed. Height-based is kept as a cheap extra.
/// Other resolutions could not be verified (no samples).
pub fn scale_candidates(c: &ClientBox) -> Vec<f64> {
    let mut v = vec![c.w as f64 / REF_CLIENT_W, 1.0, c.h as f64 / 1080.0];
    v.retain(|s| *s > 0.3 && *s < 4.0);
    let mut out: Vec<f64> = Vec::new();
    for s in v {
        if !out.iter().any(|o| (o - s).abs() < 0.01) {
            out.push(s);
        }
    }
    out
}

/// Overlay metrics (all in physical pixels) for a monitor of height `mon_h`.
#[derive(Clone, Copy, Debug)]
pub struct OverlayMetrics {
    pub scale: f64,
    pub pad: i32,
    pub radius: i32,
    pub font_px: i32,
    pub line_h: i32,
    pub thumb_w: i32,
    pub thumb_h: i32,
    pub margin: i32,
    pub gap: i32,
}

pub fn overlay_metrics(mon_h: i32, user_scale: f64, margin_ref: i32) -> OverlayMetrics {
    let s = ((mon_h as f64 / 1080.0) * user_scale).clamp(0.5, 4.0);
    let r = |v: f64| (v * s).round() as i32;
    let thumb_w = r(160.0);
    let thumb_h = (thumb_w as f64 * PANEL_H / PANEL_W).round() as i32;
    OverlayMetrics {
        scale: s,
        pad: r(14.0),
        radius: r(12.0),
        font_px: r(20.0),
        line_h: r(26.0),
        thumb_w,
        thumb_h,
        margin: r(margin_ref as f64),
        gap: r(10.0),
    }
}

/// Overlay window rectangle: vertically centred on the monitor, flush right with margin.
pub fn overlay_rect(mon: Rect, w: i32, h: i32, margin: i32) -> Rect {
    Rect::new(mon.right() - margin - w, mon.y + (mon.h - h) / 2, w, h)
}
