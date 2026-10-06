use results_watcher::config::Config;
use results_watcher::detect::*;
use results_watcher::esc::EscCounter;
use results_watcher::geometry::*;
use results_watcher::pixels::*;
use results_watcher::png_out;
use results_watcher::state::*;
use std::cell::RefCell;
use std::path::Path;

fn fx(name: &str) -> Image {
    Image::from_png_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap()
}

// Client boxes measured on the screenshots: fullscreen 1920x1080; windowed = title bar 23 px
// at the top, taskbar from y=1032 -> client 1920x1009 starting at y=23.
const FULL: ClientBox = ClientBox { x: 0, y: 0, w: 1920, h: 1080 };
const WIN: ClientBox = ClientBox { x: 0, y: 23, w: 1920, h: 1009 };

fn detect(img: &Image, c: &ClientBox) -> Option<Detection> {
    for s in scale_candidates(c) {
        let l = Layout::predicted(c, s);
        if let Some(d) = stage2(img, &l) {
            if confirm_template(img, &l) {
                return Some(d);
            }
        }
    }
    None
}

#[test]
fn predicted_origin_matches_measurements() {
    let l = Layout::predicted(&FULL, 1.0);
    assert_eq!(l.panel_rect(), Rect::new(620, 278, 719, 525));
    let l = Layout::predicted(&WIN, 1.0);
    assert_eq!(l.panel_rect(), Rect::new(620, 265, 719, 525));
}

#[test]
fn scale_is_width_based_not_height_based() {
    // 1920x1009 client must still try scale 1.0 first.
    assert!((scale_candidates(&WIN)[0] - 1.0).abs() < 1e-9);
    let c = ClientBox { x: 0, y: 0, w: 2560, h: 1440 };
    assert!((scale_candidates(&c)[0] - 4.0 / 3.0).abs() < 1e-9);
}

#[test]
fn detects_all_reference_screens() {
    let cases = [
        ("victory_full.png", FULL, Banner::Victory),
        ("victory_windowed.png", WIN, Banner::Victory),
        ("defeat_full.png", FULL, Banner::Defeat),
        ("defeat_windowed.png", WIN, Banner::Defeat),
    ];
    for (f, c, b) in cases {
        let d = detect(&fx(f), &c).unwrap_or_else(|| panic!("{} not detected", f));
        assert_eq!(d.banner, Some(b), "{}", f);
        assert!(d.rows, "{} rows", f);
        assert!((d.layout.scale - 1.0).abs() < 1e-9);
    }
}

#[test]
fn detects_resampled_panel_crop() {
    // panel.png is a 715x526 resampled crop: scale ~0.998, origin (0,0).
    let img = fx("panel_crop.png");
    let l = Layout::from_origin(0.998, 0.0, 0.0);
    let d = stage2(&img, &l).expect("crop stage2");
    assert_eq!(d.banner, Some(Banner::Victory));
    assert!(confirm_template(&img, &l), "crop template");
}

struct Recording<'a> {
    img: &'a Image,
    allowed: Vec<Rect>,
    violations: RefCell<u32>,
}
impl Sampler for Recording<'_> {
    fn get(&self, x: i32, y: i32) -> Option<[u8; 3]> {
        if !self.allowed.iter().any(|r| r.contains(x, y)) {
            *self.violations.borrow_mut() += 1;
        }
        self.img.get(x, y)
    }
}

#[test]
fn stage2_and_stage3_only_read_declared_probe_rects() {
    let img = fx("victory_full.png");
    let l = Layout::predicted(&FULL, 1.0);
    let mut allowed = probes(&l);
    allowed.push(template_probe(&l));
    let rec = Recording { img: &img, allowed: allowed.clone(), violations: RefCell::new(0) };
    assert!(stage2(&rec, &l).is_some());
    assert!(confirm_template(&rec, &l));
    assert_eq!(*rec.violations.borrow(), 0);
    let total: i64 = allowed.iter().map(|r| (r.w * r.h) as i64).sum();
    assert!(total < 12_000, "probe area {} px must stay tiny", total);
}

fn draw_cursor(img: &mut Image, x: i32, y: i32) {
    // white arrow-ish triangle with black outline, ~12x19 px
    for dy in 0..19 {
        for dx in 0..=(dy.min(12) / 2 + dy / 4) {
            img.set(x + dx, y + dy, [255, 255, 255]);
        }
        img.set(x - 1, y + dy, [0, 0, 0]);
    }
}

#[test]
fn survives_cursor_on_close_button() {
    for (f, c) in [("victory_full.png", FULL), ("defeat_windowed.png", WIN)] {
        let mut img = fx(f);
        let l = Layout::predicted(&c, 1.0);
        let x = l.rect(CLOSE_X);
        draw_cursor(&mut img, x.x + 15, x.y + 10);
        let d = detect(&img, &c).unwrap_or_else(|| panic!("{} lost with cursor", f));
        assert!(d.banner.is_some());
    }
}

#[test]
fn no_false_positive_on_colour_only_screens() {
    let (w, h) = (1920usize, 1080usize);
    for colour in [[30u8, 214, 158], [214, 37, 31], [10, 10, 10], [200, 200, 200]] {
        let mut img = Image::new(w, h);
        for px in img.rgb.chunks_exact_mut(3) {
            px.copy_from_slice(&colour);
        }
        assert!(detect(&img, &FULL).is_none(), "solid {:?}", colour);
        assert!(search_multiscale(&img).is_none());
    }
    // Panel wiped out: reference screen with the panel area repainted with background colour.
    let mut img = fx("victory_full.png");
    let l = Layout::predicted(&FULL, 1.0);
    let r = l.panel_rect();
    for y in r.y..r.bottom() {
        for x in r.x..r.right() {
            img.set(x, y, [60, 110, 140]);
        }
    }
    assert!(detect(&img, &FULL).is_none());
    // Only a green/red banner outline drawn on a dark screen (no pills / header): must fail.
    let mut img = Image::new(w, h);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            img.set(x, y, [12, 12, 12]);
        }
    }
    let b = l.rect(BANNER);
    for x in b.x..b.right() {
        img.set(x, b.y, [30, 214, 158]);
        img.set(x, b.bottom() - 1, [30, 214, 158]);
    }
    assert!(detect(&img, &FULL).is_none());
}

#[test]
fn rewards_only_is_not_a_stats_panel() {
    // Swap the two pills' colours on the reference to emulate "Rewards active": detection must
    // report Rewards (never Stats), so the state machine does not capture.
    let mut img = fx("victory_full.png");
    let l = Layout::predicted(&FULL, 1.0);
    let st = l.rect(STATS_PILL).grow(2);
    let rw = l.rect(REWARDS_PILL).grow(2);
    for y in st.y..st.bottom() {
        for x in st.x..st.right() {
            img.set(x, y, [150, 152, 154]); // grey
        }
    }
    for y in rw.y..rw.bottom() {
        for x in rw.x..rw.right() {
            img.set(x, y, [30, 214, 158]); // turquoise
        }
    }
    // Rewards active (or any non-Stats pill state) must NOT be a detection at all.
    assert!(stage2(&img, &l).is_none());
    // ...but the panel is still known to be open (header is tab-independent).
    assert!(panel_present(&img, &l));
}

#[test]
fn panel_present_only_when_panel_is_there() {
    let img = fx("defeat_full.png");
    let l = Layout::predicted(&FULL, 1.0);
    assert!(panel_present(&img, &l));
    let mut gone = img.clone();
    let r = l.panel_rect();
    for y in r.y..r.bottom() {
        for x in r.x..r.right() {
            gone.set(x, y, [60, 110, 140]);
        }
    }
    assert!(!panel_present(&gone, &l));
}

fn downscale(img: &Image, f: f64) -> Image {
    let (w, h) = ((img.w as f64 * f) as usize, (img.h as f64 * f) as usize);
    let mut out = Image::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (x0, x1) = ((x as f64 / f) as usize, (((x + 1) as f64 / f) as usize).min(img.w));
            let (y0, y1) = ((y as f64 / f) as usize, (((y + 1) as f64 / f) as usize).min(img.h));
            let mut acc = [0u32; 3];
            let mut n = 0;
            for yy in y0..y1.max(y0 + 1) {
                for xx in x0..x1.max(x0 + 1) {
                    if let Some(p) = img.get(xx as i32, yy as i32) {
                        for c in 0..3 {
                            acc[c] += p[c] as u32;
                        }
                        n += 1;
                    }
                }
            }
            if n > 0 {
                out.set(x as i32, y as i32, [(acc[0] / n) as u8, (acc[1] / n) as u8, (acc[2] / n) as u8]);
            }
        }
    }
    out
}

#[test]
fn fallback_search_finds_unscaled_and_synthetic_scaled_panel() {
    let img = fx("victory_full.png");
    let d = search_multiscale(&img).expect("search at 1.0");
    assert!((d.layout.scale - 1.0).abs() < 0.02);
    assert!((d.layout.ox - 620.0).abs() <= 1.5 && (d.layout.oy - 278.0).abs() <= 1.5, "{:?}", d.layout);
    // SYNTHETIC (not a real capture): box-downscaled 0.75x. Only proves scale-free search logic.
    let small = downscale(&img, 0.75);
    let d = search_multiscale(&small).expect("search at 0.75 (synthetic)");
    assert!((d.layout.scale - 0.75).abs() < 0.03, "{:?}", d.layout);
}

// ---------------- state machine ----------------
fn stats(b: Banner, rows: bool) -> Obs {
    Obs::Stats { banner: b, rows }
}

#[test]
fn state_one_capture_per_opening_and_close_cycle() {
    let mut m = Machine::new(Params::default());
    let mut t = 0u64;
    let mut step = |m: &mut Machine, o: Obs| {
        t += 330;
        m.on_observation(o, t)
    };
    assert_eq!(step(&mut m, Obs::Absent), Action::None);
    assert_eq!(step(&mut m, stats(Banner::Victory, true)), Action::Capture); // immediate, first observation
    assert_eq!(m.capture_result(true), Action::None);
    assert_eq!(m.state, State::Captured);
    for _ in 0..20 {
        assert_eq!(step(&mut m, stats(Banner::Victory, true)), Action::None);
    }
    // tab switch to Rewards and back: no new capture
    for _ in 0..5 {
        assert_eq!(step(&mut m, Obs::Other), Action::None);
    }
    assert_eq!(step(&mut m, stats(Banner::Victory, true)), Action::None);
    // a single cursor blip does not close
    assert_eq!(step(&mut m, Obs::Absent), Action::None);
    assert_eq!(step(&mut m, stats(Banner::Victory, true)), Action::None);
    // 3 absent in a row -> hide
    assert_eq!(step(&mut m, Obs::Absent), Action::None);
    assert_eq!(step(&mut m, Obs::Absent), Action::None);
    assert_eq!(step(&mut m, Obs::Absent), Action::HideOverlay);
    // ready for the next game without restart
    assert_eq!(step(&mut m, stats(Banner::Defeat, true)), Action::Capture);
    assert_eq!(m.state, State::Capturing);
}

#[test]
fn rewards_tab_never_captures_until_stats() {
    let mut m = Machine::new(Params::default());
    for i in 0..30 {
        let o = Obs::Other;
        assert_eq!(m.on_observation(o, i * 330), Action::None);
    }
    assert_eq!(m.state, State::Searching);
}

#[test]
fn capture_retries_up_to_three_times_then_stops() {
    let mut m = Machine::new(Params::default());
    let mut t = 0;
    t += 330;
    let act = m.on_observation(stats(Banner::Victory, true), t);
    assert_eq!(act, Action::Capture);
    assert_eq!(m.capture_result(false), Action::Capture);
    assert_eq!(m.capture_result(false), Action::Capture);
    assert_eq!(m.capture_result(false), Action::None);
    assert_eq!(m.state, State::Captured); // no endless retry loop on an unreadable panel
}

#[test]
fn banner_flip_without_closing_is_a_new_game() {
    let mut m = Machine::new(Params::default());
    let mut t = 0;
    t += 330;
    assert_eq!(m.on_observation(stats(Banner::Victory, true), t), Action::Capture);
    m.capture_result(true);
    // 1-2 flips are noise, 3 in a row restart the search
    for _ in 0..2 {
        t += 330;
        m.on_observation(stats(Banner::Defeat, true), t);
    }
    assert_eq!(m.state, State::Captured);
    t += 330;
    m.on_observation(stats(Banner::Defeat, true), t);
    assert_eq!(m.state, State::Searching);
}

// ---------------- Esc x3 ----------------
#[test]
fn esc_three_distinct_presses_trigger() {
    let mut e = EscCounter::new(3, 1200);
    assert!(!e.on_key(true, 0));
    assert!(!e.on_key(false, 50));
    assert!(!e.on_key(true, 400));
    assert!(!e.on_key(false, 450));
    assert!(e.on_key(true, 900));
    assert!(e.is_locked());
    assert!(!e.on_key(false, 950));
    assert!(!e.on_key(true, 1000)); // ignored while shutting down
}

#[test]
fn esc_held_key_and_one_or_two_presses_do_nothing() {
    let mut e = EscCounter::new(3, 1200);
    for t in 0..100 {
        assert!(!e.on_key(true, t * 30)); // auto-repeat, never released
    }
    let mut e = EscCounter::new(3, 1200);
    e.on_key(true, 0);
    e.on_key(false, 10);
    e.on_key(true, 100);
    e.on_key(false, 110);
    assert!(!e.is_locked());
}

#[test]
fn esc_gap_over_limit_resets() {
    let mut e = EscCounter::new(3, 1200);
    e.on_key(true, 0);
    e.on_key(false, 10);
    e.on_key(true, 500);
    e.on_key(false, 510);
    assert!(!e.on_key(true, 500 + 1300)); // too slow: counter restarts at 1
    e.on_key(false, 2400);
    assert!(!e.on_key(true, 2600));
    e.on_key(false, 2610);
    assert!(e.on_key(true, 2900));
}

// ---------------- misc ----------------
#[test]
fn overlay_geometry_flush_right_centered() {
    let m = overlay_metrics(1080, 1.0, 24);
    assert_eq!((m.thumb_w, m.margin), (160, 24));
    assert_eq!(m.thumb_h, 117);
    let r = overlay_rect(Rect::new(0, 0, 1920, 1080), 200, 210, m.margin);
    assert_eq!((r.x, r.y), (1920 - 24 - 200, 435));
    let m2 = overlay_metrics(1440, 1.0, 24);
    assert_eq!(m2.thumb_w, 213);
}

#[test]
fn config_defaults_and_parse() {
    let c = Config::parse("");
    assert_eq!(c.esc_count, 3);
    assert!(!c.esc_only_when_roblox_focused);
    let c = Config::parse("# x\noutput_dir = \"C:/shots\"\npoll_hz = 2\nesc_only_when_roblox_focused = true\n");
    assert_eq!(c.output_dir, "C:/shots");
    assert_eq!(c.poll_hz, 2.0);
    assert!(c.esc_only_when_roblox_focused);
}

#[test]
fn png_sequence_atomic_no_overwrite() {
    let dir = std::env::temp_dir().join(format!("rw_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("notes.txt"), b"x").unwrap();
    let rgb = vec![128u8; 4 * 3 * 3];
    let stem = png_out::screenshot_stem(2026, 10, 6, 7, 3, 59);
    assert_eq!(stem, "Screenshot_2026-10-06_070359");
    let p = png_out::save_rgb_atomic(&dir, 4, 3, &rgb, &stem).unwrap();
    assert!(p.ends_with("Screenshot_2026-10-06_070359.png"));
    let p2 = png_out::save_rgb_atomic(&dir, 4, 3, &rgb, &stem).unwrap(); // same second: never overwrite
    assert!(p2.ends_with("Screenshot_2026-10-06_070359 (2).png"));
    assert!(!std::fs::read_dir(&dir).unwrap().flatten().any(|e| e.file_name().to_string_lossy().ends_with(".tmp")));
    let back = Image::from_png_file(&p).unwrap();
    assert_eq!((back.w, back.h), (4, 3));
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------- overlay rendering (pure part) ----------------
use results_watcher::render::*;

#[test]
fn rounded_rect_is_antialiased_and_translucent() {
    let mut c = Canvas::new(100, 60);
    c.fill_rounded(12.0, [24, 25, 28], 0.88);
    let a = |x: usize, y: usize| c.px[(y * 100 + x) * 4 + 3];
    assert_eq!(a(0, 0), 0, "corner pixel is outside the rounded shape");
    assert!(a(50, 30) >= 224 && a(50, 30) <= 226, "centre alpha ~0.88*255");
    // premultiplied: colour never exceeds alpha
    assert!(c.px[(30 * 100 + 50) * 4 + 2] <= a(50, 30));
}

#[test]
fn thumbnail_keeps_aspect_ratio_and_averages() {
    let src: Vec<u8> = (0..719 * 525).flat_map(|i| if (i % 719) < 360 { [0u8, 0, 255, 255] } else { [255u8, 0, 0, 255] }).collect();
    let m = overlay_metrics(1080, 1.0, 24);
    let t = box_downscale_bgra(&src, 719, 525, m.thumb_w as usize, m.thumb_h as usize);
    assert_eq!((t.w, t.h), (160, 117));
    assert!(t.bgra[2] > 240 && t.bgra[(t.w - 1) * 4] > 240); // red left, blue right
    let ratio = t.w as f64 / t.h as f64;
    assert!((ratio - 719.0 / 525.0).abs() < 0.02);
}

#[test]
fn capture_is_immediate_and_needs_stats_with_a_player_row() {
    let mut m = Machine::new(Params::default());
    // Stats tab but no player row yet: wait, never capture.
    assert_eq!(m.on_observation(stats(Banner::Victory, false), 0), Action::None);
    // Rewards tab: never captures.
    for i in 0..50 {
        assert_eq!(m.on_observation(Obs::Other, 1000 + i * 330), Action::None);
    }
    assert_eq!(m.state, State::Searching);
    // First Stats observation with a row: captured at once.
    assert_eq!(m.on_observation(stats(Banner::Victory, true), 20_000), Action::Capture);
}

#[test]
fn debug_image_reports_reference_files() {
    let d = |f: &str, c| results_watcher::debug_image::run(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(f), c);
    let s = d("victory_full.png", Some(FULL));
    assert!(s.contains("STATS panel detected") && s.contains("Victory") && s.contains("x=620 y=278 w=719 h=525"), "{}", s);
    let s = d("defeat_windowed.png", Some(WIN));
    assert!(s.contains("Defeat") && s.contains("y=265"), "{}", s);
    let s = d("panel_crop.png", None);
    assert!(s.contains("Victory"), "{}", s);
}
