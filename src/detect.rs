//! Detection cascade, cheapest first. Everything here is pure: it only calls
//! `Sampler::get`, and only inside the rectangles returned by `probes()` / `template_probe()`,
//! so the Windows layer can fetch just those few pixels from the GPU.
//!
//! Stage 2: ~60 pixels at the predicted location (colour + structure, several criteria at once).
//! Stage 3: tiny template match of the header (icon + "RESULTS") on ~7k pixels.
//! Stage 4: rare full-frame, scale-free search of the banner outline.

use crate::geometry::*;
use crate::pixels::*;
use crate::template::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Banner {
    Victory,
    Defeat,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Detection {
    pub layout: Layout,
    /// Banner colour. A `Detection` only ever exists when the STATS tab is active:
    /// anything else (Rewards tab, unknown pill state) is simply "not detected".
    pub banner: Option<Banner>,
    /// At least one player band (green/red) visible in the table.
    pub rows: bool,
    /// Per-row colour classes (2 bits per row, 7 rows) + banner: the "content signature".
    pub sig: u32,
}

#[derive(Clone, Copy)]
enum K {
    BannerTop,
    BannerBottom,
    BannerFill,
    StatsTop,
    StatsBottom,
    StatsLeft,
    StatsFill,
    RewTop,
    RewBottom,
    RewFill,
    Dark,
    Row(u8),
}

struct Probe {
    r: Rect,
    k: K,
}

fn vstrip(l: &Layout, rx: i32, y0: i32, y1: i32) -> Rect {
    let (x, ya) = l.pt(rx as f64, y0 as f64);
    let (_, yb) = l.pt(rx as f64, y1 as f64);
    Rect::new(x, ya, 1, (yb - ya + 1).max(1))
}
fn hstrip(l: &Layout, x0: i32, x1: i32, ry: i32) -> Rect {
    let (xa, y) = l.pt(x0 as f64, ry as f64);
    let (xb, _) = l.pt(x1 as f64, ry as f64);
    Rect::new(xa, y, (xb - xa + 1).max(1), 1)
}
fn dot(l: &Layout, rx: i32, ry: i32) -> Rect {
    let (x, y) = l.pt(rx as f64, ry as f64);
    Rect::new(x, y, 1, 1)
}

fn plan(l: &Layout) -> Vec<Probe> {
    let mut v = Vec::with_capacity(48);
    for &x in &[120, 260, 400, 540, 620] {
        v.push(Probe { r: vstrip(l, x, 139, 148), k: K::BannerTop });
    }
    for &x in &[150, 400, 600] {
        v.push(Probe { r: vstrip(l, x, 196, 210), k: K::BannerBottom });
    }
    for &(x, y) in &[(150, 160), (550, 160), (150, 190), (550, 190)] {
        v.push(Probe { r: dot(l, x, y), k: K::BannerFill });
    }
    v.push(Probe { r: vstrip(l, 420, 87, 95), k: K::StatsTop });
    v.push(Probe { r: vstrip(l, 420, 116, 124), k: K::StatsBottom });
    v.push(Probe { r: hstrip(l, 380, 388, 106), k: K::StatsLeft });
    v.push(Probe { r: dot(l, 392, 106), k: K::StatsFill });
    v.push(Probe { r: vstrip(l, 300, 86, 94), k: K::RewTop });
    v.push(Probe { r: vstrip(l, 300, 118, 126), k: K::RewBottom });
    v.push(Probe { r: dot(l, 270, 106), k: K::RewFill });
    for &(x, y) in &[(300, 40), (500, 40), (10, 100), (710, 100), (10, 500), (708, 500)] {
        v.push(Probe { r: dot(l, x, y), k: K::Dark });
    }
    for i in 0..7u8 {
        let y0 = ROW_FIRST_Y + ROW_H * i as i32 + 4;
        v.push(Probe { r: vstrip(l, ROW_X + 8, y0, y0 + 30), k: K::Row(i) });
    }
    v
}

/// All frame rectangles read by `stage2`.
pub fn probes(l: &Layout) -> Vec<Rect> {
    plan(l).into_iter().map(|p| p.r).collect()
}

fn any_in<S: Sampler + ?Sized>(s: &S, r: Rect, f: impl Fn([u8; 3]) -> bool) -> bool {
    for y in r.y..r.bottom() {
        for x in r.x..r.right() {
            if let Some(p) = s.get(x, y) {
                if f(p) {
                    return true;
                }
            }
        }
    }
    false
}

/// Stage 2: cheap structural test at a predicted layout. Several criteria must hold together.
pub fn stage2<S: Sampler + ?Sized>(s: &S, l: &Layout) -> Option<Detection> {
    let (mut dark, mut dark_n) = (0, 0);
    let (mut st_top, mut st_bot, mut st_left, mut st_fill) = (false, false, false, false);
    let (mut rw_top_gray, mut rw_bot_gray) = (false, false);
    let (mut rw_top_t, mut rw_bot_t, mut rw_fill_t) = (false, false, false);
    let (mut bt_g, mut bt_r, mut bb_g, mut bb_r, mut bf_g, mut bf_r) = (0, 0, 0, 0, 0, 0);
    let mut rowcls = [0u32; 7];

    for p in plan(l) {
        match p.k {
            K::BannerTop => {
                bt_g += any_in(s, p.r, is_turq) as i32;
                bt_r += any_in(s, p.r, is_red_line) as i32;
            }
            K::BannerBottom => {
                bb_g += any_in(s, p.r, is_turq) as i32;
                bb_r += any_in(s, p.r, is_red_line) as i32;
            }
            K::BannerFill => {
                bf_g += any_in(s, p.r, is_team_green) as i32;
                bf_r += any_in(s, p.r, is_team_red) as i32;
            }
            K::StatsTop => {
                st_top = any_in(s, p.r, is_turq);
            }
            K::StatsBottom => st_bot = any_in(s, p.r, is_turq),
            K::StatsLeft => st_left = any_in(s, p.r, is_turq),
            K::StatsFill => st_fill = any_in(s, p.r, is_team_green), // active pill interior is dim teal (12,50,42)
            K::RewTop => {
                rw_top_t = any_in(s, p.r, is_turq);
                rw_top_gray = any_in(s, p.r, is_gray_line);
            }
            K::RewBottom => {
                rw_bot_t = any_in(s, p.r, is_turq);
                rw_bot_gray = any_in(s, p.r, is_gray_line);
            }
            K::RewFill => rw_fill_t = any_in(s, p.r, is_turq),
            K::Dark => {
                dark_n += 1;
                if any_in(s, p.r, is_dark) {
                    dark += 1;
                }
            }
            K::Row(i) => {
                rowcls[i as usize] = if any_in(s, p.r, is_team_green) {
                    1
                } else if any_in(s, p.r, is_team_red) {
                    2
                } else {
                    0
                };
            }
        }
    }

    // Criterion A: dark panel body at >= 5 of the 6 empty spots.
    if dark_n == 0 || dark < 5 {
        return None;
    }
    // Criterion B: STATS tab active = Stats pill turquoise AND Rewards pill grey-outlined.
    // Any other pill state (including Rewards active) is NOT a detection: nothing is captured.
    let stats_t = st_top && st_bot && st_left && st_fill;
    let rew_g = rw_top_gray && rw_bot_gray && !rw_fill_t && !(rw_top_t && rw_bot_t);
    if !(stats_t && rew_g) {
        return None;
    }
    // Criterion C: banner outline + fill, same colour family.
    let banner = {
        let green = bt_g >= 4 && bb_g >= 1 && bf_g >= 3;
        let red = bt_r >= 4 && bb_r >= 1 && bf_r >= 3;
        match (green, red) {
            (true, false) => Banner::Victory,
            (false, true) => Banner::Defeat,
            _ => return None,
        }
    };
    let rows = rowcls[0] != 0;
    let mut sig = 0u32;
    for (i, c) in rowcls.iter().enumerate() {
        sig |= c << (2 * i);
    }
    sig |= match banner {
        Banner::Victory => 1 << 16,
        Banner::Defeat => 2 << 16,
    };
    Some(Detection { layout: *l, banner: Some(banner), rows, sig })
}

/// Rectangle (frame pixels) read by `confirm_template`.
pub fn template_probe(l: &Layout) -> Rect {
    l.rect(TITLE_ICON).grow(3)
}

fn bit(mask: &[u8], i: usize) -> bool {
    mask[i >> 3] >> (i & 7) & 1 == 1
}

/// Stage 3: mini template matching of the header (icon + title) on a tiny zone, tolerant to a
/// +-2 px prediction error. Works at the deduced scale by nearest-neighbour mapping.
pub fn confirm_template<S: Sampler + ?Sized>(s: &S, l: &Layout) -> bool {
    template_scores(s, l).map_or(false, |b| template_pass(l, b))
}

/// (on-rate, off-rate, icon on-rate, icon off-rate, best dx, best dy) of the best +-2 px alignment.
pub fn template_scores<S: Sampler + ?Sized>(s: &S, l: &Layout) -> Option<(f32, f32, f32, f32, i32, i32)> {
    let mut best = (0f32, 0f32, 0f32, 0f32, 0i32, 0i32);
    for dy in -2i32..=2 {
        for dx in -2i32..=2 {
            let (mut on_h, mut on_n, mut off_h, mut off_n) = (0u32, 0u32, 0u32, 0u32);
            let (mut ion_h, mut ion_n, mut ioff_h, mut ioff_n) = (0u32, 0u32, 0u32, 0u32);
            for ty in 0..TPL_H {
                let fy = (l.oy + (TPL_Y as f64 + ty as f64 + 0.5) * l.scale).floor() as i32 + dy;
                for tx in 0..TPL_W {
                    let i = ty * TPL_W + tx;
                    let on = bit(&TPL_ON, i);
                    let off = bit(&TPL_OFF, i);
                    if !on && !off {
                        continue;
                    }
                    let fx = (l.ox + (TPL_X as f64 + tx as f64 + 0.5) * l.scale).floor() as i32 + dx;
                    let lm = match s.get(fx, fy) {
                        Some(p) => luma(p),
                        None => return None,
                    };
                    let icon = tx < 44;
                    if on {
                        on_n += 1;
                        let hit = lm >= 130.0;
                        on_h += hit as u32;
                        if icon {
                            ion_n += 1;
                            ion_h += hit as u32;
                        }
                    } else {
                        off_n += 1;
                        let hit = lm <= 110.0;
                        off_h += hit as u32;
                        if icon {
                            ioff_n += 1;
                            ioff_h += hit as u32;
                        }
                    }
                }
            }
            let r = |h: u32, n: u32| if n == 0 { 0.0 } else { h as f32 / n as f32 };
            let cand = (r(on_h, on_n), r(off_h, off_n), r(ion_h, ion_n), r(ioff_h, ioff_n), dx, dy);
            if cand.0 + cand.1 > best.0 + best.1 {
                best = cand;
            }
        }
    }
    Some(best)
}

pub fn template_pass(l: &Layout, best: (f32, f32, f32, f32, i32, i32)) -> bool {
    let near1 = (l.scale - 1.0).abs() < 0.03;
    let (on_thr, off_thr) = if near1 { (0.80, 0.90) } else { (0.55, 0.85) };
    let full = best.0 >= on_thr && best.1 >= off_thr;
    // Language/theme tolerance: the icon alone is enough if it matches very well.
    let icon_only = best.2 >= 0.88 && best.3 >= 0.92;
    full || icon_only
}

/// Stage 4 (rare): scale-free search of the banner outline on a full frame.
/// Finds two long horizontal border runs of the same colour/length/x, ~61*scale apart,
/// deduces scale and origin from them, then validates with stage 2.
pub fn search_multiscale(img: &Image) -> Option<Detection> {
    struct Run {
        y: i32,
        x0: i32,
        len: i32,
        green: bool,
    }
    let mut runs: Vec<Run> = Vec::new();
    for y in 0..img.h as i32 {
        let mut x = 0i32;
        while (x as usize) < img.w {
            let p = img.get(x, y).unwrap();
            if let Some(g) = is_border_strict(p) {
                let x0 = x;
                x += 1;
                while (x as usize) < img.w && is_border_strict(img.get(x, y).unwrap()) == Some(g) {
                    x += 1;
                }
                if x - x0 >= 150 {
                    // Keep only the first row of a multi-row (thick) line.
                    let dup = runs.last().map_or(false, |r| {
                        r.y + 1 == y && r.green == g && (r.x0 - x0).abs() <= 2 && (r.len - (x - x0)).abs() <= 3
                    });
                    if !dup {
                        runs.push(Run { y, x0, len: x - x0, green: g });
                    }
                }
            } else {
                x += 1;
            }
        }
    }
    // A real panel gives a handful of long lines. Hundreds mean a solid-colour scene: not a panel.
    if runs.len() > 60 {
        return None;
    }
    let mut tried = 0;
    for top in runs.iter() {
        // Banner top border run is ~645 ref px long (655 minus rounded corners): rough scale only.
        // The bottom border is NOT required here (it can be split over two rows when the UI is
        // scaled); `stage2` validates the whole structure instead.
        let s0 = top.len as f64 / 645.0;
        if !(0.3..=4.0).contains(&s0) {
            continue;
        }
        tried += 1;
        if tried > 8 {
            return None;
        }
        // Refine scale (+-4 %) and origin (+-3 px) with the structural test + template score.
        let mut best: Option<(f32, Detection, i32, i32)> = None;
        for si in -10..=10 {
            let s = s0 * (1.0 + 0.004 * si as f64);
            let (ox, oy) = (top.x0 as f64 - 36.0 * s, top.y as f64 - 143.0 * s);
            for ddy in -3..=3 {
                for ddx in -3..=3 {
                    let l = Layout::from_origin(s, ox + ddx as f64, oy + ddy as f64);
                    let Some(d) = stage2(img, &l) else { continue };
                    let Some(sc) = template_scores(img, &l) else { continue };
                    if !template_pass(&l, sc) {
                        continue;
                    }
                    // Prefer the alignment where the template matches without any shift.
                    let score = sc.0 + sc.1 - 0.01 * (sc.4.abs() + sc.5.abs()) as f32;
                    if best.as_ref().map_or(true, |b| score > b.0) {
                        best = Some((score, d, sc.4, sc.5));
                    }
                }
            }
        }
        if let Some((_, d, sx, sy)) = best {
            // Snap the origin by the residual template shift, then re-validate.
            let l2 = Layout::from_origin(d.layout.scale, d.layout.ox + sx as f64, d.layout.oy + sy as f64);
            return Some(stage2(img, &l2).unwrap_or(d));
        }
    }
    None
}

/// Tab-agnostic "is the RESULTS panel still open?" test, used only to tell "user switched tab"
/// from "panel closed" once a capture has been made. Dark body + header template (icon and
/// title do not depend on the active tab). ASSUMPTION: the header is identical on the Rewards tab
/// (no sample available). It can never trigger a capture.
pub fn panel_present<S: Sampler + ?Sized>(s: &S, l: &Layout) -> bool {
    let mut dark = 0;
    for &(x, y) in &[(300, 40), (500, 40), (10, 100), (710, 100), (10, 500), (708, 500)] {
        let (fx, fy) = l.pt(x as f64, y as f64);
        if s.get(fx, fy).map_or(false, is_dark) {
            dark += 1;
        }
    }
    dark >= 5 && confirm_template(s, l)
}

/// Frame rectangles read by `panel_present`.
pub fn presence_probes(l: &Layout) -> Vec<Rect> {
    let mut v: Vec<Rect> = [(300, 40), (500, 40), (10, 100), (710, 100), (10, 500), (708, 500)]
        .iter()
        .map(|&(x, y)| {
            let (fx, fy) = l.pt(x as f64, y as f64);
            Rect::new(fx, fy, 1, 1)
        })
        .collect();
    v.push(template_probe(l));
    v
}
