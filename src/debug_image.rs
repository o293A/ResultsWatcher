//! `--debug-image <file.png> [--client x,y,w,h]`: run the detection on a PNG, capture nothing.
//! Pure (no OS calls) so it is also testable on any platform.

use crate::detect::*;
use crate::geometry::*;
use crate::pixels::*;
use std::path::Path;

pub fn run(path: &Path, client: Option<ClientBox>) -> String {
    let img = match Image::from_png_file(path) {
        Ok(i) => i,
        Err(e) => return format!("ERROR: cannot read {}: {}\n", path.display(), e),
    };
    let mut out = format!("image: {} ({}x{})\n", path.display(), img.w, img.h);
    let (w, h) = (img.w as f64, img.h as f64);
    let layouts: Vec<Layout> = if (w - PANEL_W).abs() < 0.1 * PANEL_W && (h - PANEL_H).abs() < 0.1 * PANEL_H {
        out += "mode: panel crop (origin 0,0)\n";
        vec![Layout::from_origin(w / PANEL_W, 0.0, 0.0)]
    } else {
        let c = client.unwrap_or(ClientBox { x: 0, y: 0, w: img.w as i32, h: img.h as i32 });
        out += &format!("mode: full frame, client box = ({},{}) {}x{}\n", c.x, c.y, c.w, c.h);
        scale_candidates(&c).into_iter().map(|s| Layout::predicted(&c, s)).collect()
    };
    let mut found: Option<(Detection, &str)> = None;
    for l in &layouts {
        if let Some(d) = stage2(&img, l) {
            if confirm_template(&img, l) {
                found = Some((d, "predicted position"));
                break;
            }
        }
    }
    if found.is_none() {
        if let Some(d) = search_multiscale(&img) {
            found = Some((d, "fallback multi-scale search"));
        }
    }
    match found {
        Some((d, how)) => {
            let r = d.layout.panel_rect();
            out += &format!(
                "RESULT: STATS panel detected via {}\n  panel rect: x={} y={} w={} h={}\n  scale: {:.3}\n  active tab: Stats\n  banner: {:?}\n  player rows visible: {}\n",
                how, r.x, r.y, r.w, r.h, d.layout.scale, d.banner.unwrap(), d.rows
            );
        }
        None => {
            let present = layouts.iter().any(|l| panel_present(&img, l));
            out += if present {
                "RESULT: panel visible but Stats tab NOT active -> no capture\n"
            } else {
                "RESULT: no RESULTS panel found -> no capture\n"
            };
        }
    }
    out
}

pub fn parse_client(s: &str) -> Option<ClientBox> {
    let v: Vec<i32> = s.split(',').filter_map(|p| p.trim().parse().ok()).collect();
    (v.len() == 4).then(|| ClientBox { x: v[0], y: v[1], w: v[2], h: v[3] })
}
