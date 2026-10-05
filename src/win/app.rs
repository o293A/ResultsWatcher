//! Event-driven application: one thread, one message loop (0 % CPU when idle).
//!  * A WinEvent hook (out-of-process notification, nothing injected into Roblox) wakes us when
//!    the foreground window / minimise state / move-size changes.
//!  * A poll timer (~3 Hz) only exists while Roblox is the visible foreground window.
//!  * Raw Input (RIDEV_INPUTSINK) on a message-only window feeds the Esc x3 counter.

use super::capture::*;
use super::overlay::{Content, Overlay};
use super::util::*;
use crate::config::Config;
use crate::detect::*;
use crate::esc::EscCounter;
use crate::geometry::*;
use crate::pixels::{is_border_strict, Image, Sampler};
use crate::state::*;
use crate::{png_out, wlog};
use std::collections::HashMap;
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, AtomicPtr, Ordering};
use std::time::Instant;
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows::Win32::UI::Input::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const WM_WAKE: u32 = WM_APP + 1;
const WM_SAVED: u32 = WM_APP + 2;
const T_POLL: usize = 1;
const T_FADE: usize = 2;
const T_TOP: usize = 3;
const T_CLOSE_SHOW: usize = 4;
const T_QUIT: usize = 5;
const FADE_IN_MS: u64 = 150;
const FADE_OUT_MS: u64 = 250;

static APP: AtomicPtr<App> = AtomicPtr::new(std::ptr::null_mut());
static CTL: AtomicIsize = AtomicIsize::new(0);

struct SavedMsg {
    ok: bool,
    path: PathBuf,
}

struct App {
    cfg: Config,
    out_dir: PathBuf,
    t0: Instant,
    ctl: HWND,
    gpu: Option<Gpu>,
    session: Option<Session>,
    /// Last delivered frame, re-analysed on every poll because WGC sends nothing on a static screen.
    last: Option<Frame>,
    source: Source,
    fails: u32,
    last_frame_ms: u64,
    atlas_tex: Option<ID3D11Texture2D>,
    mach: Machine,
    esc: EscCounter,
    overlay: Overlay,
    polling: bool,
    cur_layout: Option<Layout>,
    /// per client size: (scale, origin relative to client box) found by the rare fallback search
    cache: HashMap<(i32, i32), (f64, f64, f64)>,
    last_search_ms: Option<u64>,
    retry_capture: bool,
    pending_thumb: Option<Image>,
    last_monitor: Rect,
    saving: Vec<std::thread::JoinHandle<()>>,
    shutting_down: bool,
}

impl App {
    fn now(&self) -> u64 {
        self.t0.elapsed().as_millis() as u64
    }

    // ---------------------------------------------------------------- wake / sleep
    /// Called on every foreground/minimise/move event: start or stop the poll timer.
    fn wake(&mut self) {
        if self.shutting_down {
            return;
        }
        let active = roblox_foreground(&self.cfg.roblox_process).is_some();
        unsafe {
            if active && !self.polling {
                let ms = (1000.0 / self.cfg.poll_hz) as u32;
                SetTimer(self.ctl, T_POLL, ms, None);
                self.polling = true;
            } else if !active && self.polling {
                let _ = KillTimer(self.ctl, T_POLL);
                self.polling = false;
                self.session = None; // stop WGC completely: no GPU work while the game is not active
                self.last = None;
            }
        }
    }

    // ---------------------------------------------------------------- one detection pass
    fn tick(&mut self) {
        if self.shutting_down {
            return;
        }
        let Some(hwnd) = roblox_foreground(&self.cfg.roblox_process) else {
            self.wake();
            return;
        };
        let Some(geo) = geometry(hwnd) else { return };
        self.last_monitor = geo.monitor;
        if self.source == Source::Duplication && self.gpu.as_ref().map_or(false, |g| g.hmon != Some(geo.hmon)) {
            // Roblox moved to another monitor: Desktop Duplication needs the GPU driving it.
            self.gpu = None;
            self.session = None;
            self.last = None;
            self.atlas_tex = None;
        }
        if self.gpu.is_none() {
            let mon = if self.source == Source::Duplication { Some(geo.hmon) } else { None };
            match Gpu::new(mon) {
                Ok(g) => self.gpu = Some(g),
                Err(e) => return self.fail(&format!("gpu init: {e}")),
            }
        }
        if self.session.is_none() {
            let gpu = self.gpu.as_ref().unwrap();
            match Session::start(gpu, self.source, hwnd, geo.hmon) {
                Ok(s) => {
                    self.session = Some(s);
                    self.last_frame_ms = self.now();
                }
                Err(e) => return self.fail(&format!("capture start ({:?}): {e}", self.source)),
            }
        }
        let gpu = self.gpu.as_ref().unwrap();
        let newest = self.session.as_mut().unwrap().latest(gpu);
        if self.session.as_ref().map_or(false, |s| s.lost()) {
            wlog!("capture access lost: rebuilding the session");
            self.session = None;
            self.last = None;
            return;
        }
        if let Some(f) = newest {
            self.last = Some(f);
            self.last_frame_ms = self.now();
            if self.fails > 0 {
                // Capture works again: leave the back-off rate.
                let ms = (1000.0 / self.cfg.poll_hz) as u32;
                unsafe {
                    SetTimer(self.ctl, T_POLL, ms, None);
                }
            }
            self.fails = 0;
        }
        // Keep re-analysing the last frame: WGC only delivers frames when the screen changes.
        let size = self.session.as_ref().unwrap().size;
        if self.last.as_ref().map_or(false, |l| (l.w, l.h) != size) {
            self.last = None; // window was resized: the old frame is stale
        }
        let Some(frame) = self.last.take() else {
            // Nothing for 10 s: rebuild the session (sleep/wake, device loss).
            if self.now().saturating_sub(self.last_frame_ms) > 10_000 {
                self.fail("no frame for 10 s");
            }
            return;
        };

        let cb = client_in_frame(&geo, self.source);
        let obs = self.observe(&frame, &cb);
        let now = self.now();
        let mut act = if self.retry_capture && self.mach.state == State::Capturing {
            self.retry_capture = false;
            Action::Capture
        } else {
            self.mach.on_observation(obs, now)
        };
        while act == Action::Capture {
            let ok = self.do_capture(&frame);
            act = self.mach.capture_result(ok);
            if act == Action::Capture && !ok {
                self.retry_capture = true; // retry with the next frame, not the same one
                break;
            }
        }
        if act == Action::HideOverlay {
            self.hide_overlay();
        }
        self.last = Some(frame);
    }

    fn fail(&mut self, why: &str) {
        self.fails += 1;
        wlog!("capture problem #{}: {}", self.fails, why);
        self.session = None;
        self.last = None;
        match (self.source, self.fails) {
            (Source::Duplication, 3) => {
                wlog!("desktop duplication keeps failing -> switching to window capture (Windows may draw a yellow border)");
                self.source = Source::Window;
                self.gpu = None;
                self.atlas_tex = None;
            }
            (Source::Window, 3) | (Source::Window, 6) => {
                wlog!("window capture keeps failing -> switching to monitor capture (cropped in memory)");
                self.source = Source::Monitor;
            }
            _ => {}
        }
        if self.fails > 6 {
            self.gpu = None; // recreate the D3D device (device lost, driver reset)
            self.atlas_tex = None;
        }
        // Progressive back-off without a busy loop: slow the poll timer (1 s .. 30 s).
        unsafe {
            if self.polling {
                let ms = (1000u32 << self.fails.min(5)).min(30_000);
                SetTimer(self.ctl, T_POLL, ms, None);
            }
        }
    }

    fn observe(&mut self, frame: &Frame, cb: &ClientBox) -> Obs {
        let gpu = self.gpu.as_ref().unwrap();
        // Candidate layouts: current (while a panel is known), cached fallback result, predictions.
        let mut cands: Vec<Layout> = Vec::new();
        if let (Some(l), true) = (self.cur_layout, self.mach.state != State::Searching) {
            cands.push(l);
        }
        if let Some(&(s, rx, ry)) = self.cache.get(&(cb.w, cb.h)) {
            cands.push(Layout::from_origin(s, cb.x as f64 + rx, cb.y as f64 + ry));
        }
        for s in scale_candidates(cb) {
            cands.push(Layout::predicted(cb, s));
        }
        for l in &cands {
            let Some(a) = Atlas::fetch(gpu, &mut self.atlas_tex, frame, &probes(l)) else { continue };
            if let Some(d) = stage2(&a, l) {
                let Some(t) = Atlas::fetch(gpu, &mut self.atlas_tex, frame, &[template_probe(l)]) else { continue };
                if confirm_template(&t, l) {
                    self.cur_layout = Some(*l);
                    return Obs::Stats { banner: d.banner.unwrap(), rows: d.rows, hdr: d.hdr };
                }
            }
        }
        // Not a Stats panel. Is the panel still open (tab switch) or gone?
        if self.mach.state != State::Searching {
            if let Some(l) = self.cur_layout {
                if let Some(a) = Atlas::fetch(gpu, &mut self.atlas_tex, frame, &presence_probes(&l)) {
                    if panel_present(&a, &l) {
                        return Obs::Other;
                    }
                }
            }
            return Obs::Absent;
        }
        // Stage 4: rare. Gate = a thin vertical strip through the screen centre must cross two
        // banner-like border lines; then a rate-limited full-frame search (cached per window size).
        if self.cfg.fallback_search && self.looks_like_panel(frame, cb) {
            let now = self.now();
            if self.last_search_ms.map_or(true, |t| now.saturating_sub(t) > 10_000) {
                self.last_search_ms = Some(now);
                if let Some(img) = read_full(self.gpu.as_ref().unwrap(), frame) {
                    if let Some(d) = search_multiscale(&img) {
                        self.cache.insert((cb.w, cb.h), (d.layout.scale, d.layout.ox - cb.x as f64, d.layout.oy - cb.y as f64));
                        self.cur_layout = Some(d.layout);
                        wlog!("fallback search found panel: scale {:.3}", d.layout.scale);
                        return Obs::Stats { banner: d.banner.unwrap(), rows: d.rows, hdr: d.hdr };
                    }
                }
            }
        }
        Obs::Absent
    }

    fn looks_like_panel(&mut self, frame: &Frame, cb: &ClientBox) -> bool {
        let gpu = self.gpu.as_ref().unwrap();
        let cx = (cb.x as f64 + cb.w as f64 / 2.0 + X_SHIFT_FRAC * cb.w as f64) as i32;
        let mut strips = Vec::new();
        let mut y = cb.y.max(0);
        let end = (cb.y + cb.h).min(frame.h);
        while y < end {
            let h = (end - y).min(150);
            strips.push(Rect::new(cx, y, 1, h));
            y += h;
        }
        let Some(a) = Atlas::fetch(gpu, &mut self.atlas_tex, frame, &strips) else { return false };
        let (mut g, mut r) = (0, 0);
        for yy in cb.y.max(0)..end {
            if let Some(p) = a.get(cx, yy) {
                match is_border_strict(p) {
                    Some(true) => g += 1,
                    Some(false) => r += 1,
                    None => {}
                }
            }
        }
        (2..=40).contains(&g) || (2..=40).contains(&r)
    }

    // ---------------------------------------------------------------- final capture
    fn do_capture(&mut self, frame: &Frame) -> bool {
        let gpu = self.gpu.as_ref().unwrap();
        let Some(l) = self.cur_layout else { return false };
        let r = l.panel_rect();
        let Some(img) = read_rect_rgb(gpu, frame, r) else { return false };
        // Sanity: plausible size, not black / empty, still recognisable as the Stats panel.
        let ratio = img.w as f64 / img.h as f64;
        if img.w < 200 || img.h < 150 || img.w > 4000 || (ratio - PANEL_W / PANEL_H).abs() > 0.08 {
            return false;
        }
        let (mut sum, mut mx, mut mn) = (0u64, 0u8, 255u8);
        for p in img.rgb.chunks_exact(3).step_by(37) {
            let v = p[0].max(p[1]).max(p[2]);
            sum += v as u64;
            mx = mx.max(v);
            mn = mn.min(v);
        }
        if sum / (img.rgb.len() as u64 / 3 / 37).max(1) < 4 || mx.saturating_sub(mn) < 40 {
            return false; // black or flat frame (first WGC frame, exclusive fullscreen...)
        }
        let crop_l = Layout::from_origin(img.w as f64 / PANEL_W, 0.0, 0.0);
        if stage2(&img, &crop_l).is_none() {
            return false;
        }
        // Thumbnail from memory (overlay proves the capture), PNG encoded on a low-priority thread.
        self.pending_thumb = Some(img.clone());
        let (w, h) = (img.w as u32, img.h as u32);
        let dir = self.out_dir.clone();
        let ctl = self.ctl.0 as isize;
        let rgb = img.rgb;
        self.saving.retain(|j| !j.is_finished());
        self.saving.push(std::thread::spawn(move || {
            lower_current_thread();
            let res = png_out::save_rgb_atomic(&dir, w, h, &rgb);
            let msg = Box::new(match res {
                Ok((_, p)) => SavedMsg { ok: true, path: p },
                Err(e) => {
                    wlog!("png save failed: {e}");
                    SavedMsg { ok: false, path: PathBuf::new() }
                }
            });
            unsafe {
                let p = Box::into_raw(msg);
                if PostMessageW(HWND(ctl as *mut c_void), WM_SAVED, WPARAM(0), LPARAM(p as isize)).is_err() {
                    drop(Box::from_raw(p));
                }
            }
        }));
        true
    }

    fn on_saved(&mut self, msg: SavedMsg) {
        if msg.ok {
            wlog!("saved {}", msg.path.display());
        }
        let Some(img) = self.pending_thumb.take() else { return };
        if !msg.ok || self.shutting_down || self.mach.state != State::Captured {
            return;
        }
        let now = self.now();
        self.overlay.show(&Content::Found(img), self.last_monitor, self.cfg.overlay_scale, self.cfg.overlay_margin, FADE_IN_MS, now);
        self.start_overlay_timers();
    }

    fn start_overlay_timers(&self) {
        unsafe {
            SetTimer(self.ctl, T_FADE, 15, None);
            SetTimer(self.ctl, T_TOP, 1000, None); // keep it above everything (fullscreen windowed games)
        }
    }

    fn hide_overlay(&mut self) {
        self.pending_thumb = None;
        let now = self.now();
        self.overlay.hide(FADE_OUT_MS, now);
        unsafe {
            SetTimer(self.ctl, T_FADE, 15, None);
        }
    }

    // ---------------------------------------------------------------- Esc x3 / shutdown
    fn on_key(&mut self, down: bool) {
        if self.shutting_down {
            return;
        }
        if self.cfg.esc_only_when_roblox_focused && roblox_foreground(&self.cfg.roblox_process).is_none() {
            return;
        }
        let now = self.now();
        if self.esc.on_key(down, now) {
            self.begin_shutdown();
        }
    }

    fn begin_shutdown(&mut self) {
        self.shutting_down = true;
        unsafe {
            let _ = KillTimer(self.ctl, T_POLL);
        }
        self.session = None;
        self.last = None;
        self.pending_thumb = None;
        let now = self.now();
        // Same place/style, text replaced immediately (no thumbnail), kept for 2 s.
        self.overlay.show(&Content::Closed, self.last_monitor, self.cfg.overlay_scale, self.cfg.overlay_margin, 0, now);
        self.start_overlay_timers();
        unsafe {
            SetTimer(self.ctl, T_CLOSE_SHOW, 2000, None);
        }
        wlog!("Esc x{} received: shutting down", self.cfg.esc_count);
    }
}

fn client_in_frame(geo: &WinGeom, src: Source) -> ClientBox {
    match src {
        Source::Window => geo.client,
        Source::Monitor | Source::Duplication => ClientBox { x: geo.frame.x + geo.client.x - geo.monitor.x, y: geo.frame.y + geo.client.y - geo.monitor.y, w: geo.client.w, h: geo.client.h },
    }
}

fn app() -> Option<&'static mut App> {
    let p = APP.load(Ordering::Relaxed);
    if p.is_null() {
        None
    } else {
        Some(unsafe { &mut *p })
    }
}

unsafe extern "system" fn winevent(_: HWINEVENTHOOK, _: u32, _: HWND, _: i32, _: i32, _: u32, _: u32) {
    let h = CTL.load(Ordering::Relaxed);
    if h != 0 {
        let _ = PostMessageW(HWND(h as *mut c_void), WM_WAKE, WPARAM(0), LPARAM(0));
    }
}

unsafe extern "system" fn ctl_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if let Some(a) = app() {
        match m {
            WM_WAKE => {
                a.wake();
                return LRESULT(0);
            }
            WM_TIMER => {
                match w.0 {
                    T_POLL => a.tick(),
                    T_FADE => {
                        let now = a.now();
                        if !a.overlay.tick(now) {
                            let _ = KillTimer(h, T_FADE);
                            if !a.overlay.is_visible() {
                                let _ = KillTimer(h, T_TOP);
                            }
                        }
                    }
                    T_TOP => {
                        if a.overlay.is_visible() {
                            a.overlay.raise();
                        }
                    }
                    T_CLOSE_SHOW => {
                        let _ = KillTimer(h, T_CLOSE_SHOW);
                        let now = a.now();
                        a.overlay.hide(FADE_OUT_MS, now);
                        SetTimer(h, T_FADE, 15, None);
                        SetTimer(h, T_QUIT, FADE_OUT_MS as u32 + 120, None);
                    }
                    T_QUIT => {
                        let _ = KillTimer(h, T_QUIT);
                        PostQuitMessage(0);
                    }
                    _ => {}
                }
                return LRESULT(0);
            }
            WM_SAVED => {
                let msg = Box::from_raw(l.0 as *mut SavedMsg);
                a.on_saved(*msg);
                return LRESULT(0);
            }
            WM_INPUT => {
                let mut ri: RAWINPUT = std::mem::zeroed();
                let mut size = std::mem::size_of::<RAWINPUT>() as u32;
                let n = GetRawInputData(HRAWINPUT(l.0 as *mut c_void), RID_INPUT, Some(&mut ri as *mut _ as *mut c_void), &mut size, std::mem::size_of::<RAWINPUTHEADER>() as u32);
                if n != u32::MAX && ri.header.dwType == RIM_TYPEKEYBOARD.0 {
                    let kb = ri.data.keyboard;
                    if kb.VKey == VK_ESCAPE.0 {
                        let up = (kb.Flags as u32 & RI_KEY_BREAK) != 0;
                        a.on_key(!up);
                    }
                }
                return DefWindowProcW(h, m, w, l); // required cleanup for INPUTSINK; the key is never blocked
            }
            _ => {}
        }
    }
    DefWindowProcW(h, m, w, l)
}

pub fn run() {
    set_dpi_awareness();
    let exe = std::env::current_exe().unwrap_or_default();
    let exe_dir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let args: Vec<String> = std::env::args().collect();

    if let Some(i) = args.iter().position(|a| a == "--debug-image") {
        let Some(p) = args.get(i + 1) else { return console_print("usage: ResultsWatcher --debug-image <file.png> [--client x,y,w,h]\n") };
        let client = args.iter().position(|a| a == "--client").and_then(|j| args.get(j + 1)).and_then(|s| crate::debug_image::parse_client(s));
        console_print(&crate::debug_image::run(std::path::Path::new(p), client));
        return;
    }
    if args.iter().any(|a| a == "--install-startup" || a == "--uninstall-startup") {
        use std::os::windows::process::CommandExt;
        let install = args.iter().any(|a| a == "--install-startup");
        let mut c = std::process::Command::new("schtasks");
        if install {
            c.args(["/Create", "/TN", "ResultsWatcher", "/TR", &format!("\"{}\"", exe.display()), "/SC", "ONLOGON", "/RL", "LIMITED", "/F"]);
        } else {
            c.args(["/Delete", "/TN", "ResultsWatcher", "/F"]);
        }
        let ok = c.creation_flags(0x0800_0000).status().map(|s| s.success()).unwrap_or(false);
        console_print(&format!("{}: {}\n", if install { "install-startup" } else { "uninstall-startup" }, if ok { "ok" } else { "failed" }));
        return;
    }

    // Single instance: a second launch exits silently.
    unsafe {
        let _mutex = windows::Win32::System::Threading::CreateMutexW(None, false, w!("Local\\ResultsWatcher_SingleInstance"));
        if windows::Win32::Foundation::GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        let cfg = Config::load(&exe_dir);
        crate::logger::init(exe_dir.join("watcher.log"), cfg.log_max_kb);
        wlog!("start");
        tune_process();

        let hi = GetModuleHandleW(None).unwrap();
        let wc = WNDCLASSW { lpfnWndProc: Some(ctl_proc), hInstance: hi.into(), lpszClassName: w!("RWControl"), ..Default::default() };
        RegisterClassW(&wc);
        // Message-only window: invisible, no taskbar entry, not in Alt+Tab.
        let ctl = CreateWindowExW(WINDOW_EX_STYLE(0), w!("RWControl"), w!(""), WINDOW_STYLE(0), 0, 0, 0, 0, HWND_MESSAGE, HMENU::default(), hi, None).unwrap();
        CTL.store(ctl.0 as isize, Ordering::Relaxed);
        let overlay = Overlay::new().unwrap();

        let dev = RAWINPUTDEVICE { usUsagePage: 1, usUsage: 6, dwFlags: RIDEV_INPUTSINK, hwndTarget: ctl };
        if RegisterRawInputDevices(&[dev], std::mem::size_of::<RAWINPUTDEVICE>() as u32).is_err() {
            wlog!("RegisterRawInputDevices failed: Esc x3 unavailable");
        }

        let prim = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(prim, &mut mi);
        let r = mi.rcMonitor;

        let params = Params { stable_checks: cfg.stable_checks, bubble_wait_ms: cfg.bubble_wait_ms, close_checks: cfg.close_checks, max_capture_attempts: 3 };
        let cfg_capture = cfg.capture.clone();
        let mut a = Box::new(App {
            out_dir: cfg.resolve_output_dir(&exe_dir),
            esc: EscCounter::new(cfg.esc_count, cfg.esc_max_gap_ms),
            cfg,
            t0: Instant::now(),
            ctl,
            gpu: None,
            session: None,
            last: None,
            source: match cfg_capture.as_str() {
                "window" => Source::Window,
                "monitor" => Source::Monitor,
                _ => Source::Duplication,
            },
            fails: 0,
            last_frame_ms: 0,
            atlas_tex: None,
            mach: Machine::new(params),
            overlay,
            polling: false,
            cur_layout: None,
            cache: HashMap::new(),
            last_search_ms: None,
            retry_capture: false,
            pending_thumb: None,
            last_monitor: Rect::new(r.left, r.top, r.right - r.left, r.bottom - r.top),
            saving: Vec::new(),
            shutting_down: false,
        });
        APP.store(&mut *a as *mut App, Ordering::Relaxed);

        // Foreground / minimise / move-size notifications (out-of-process, no injection).
        let hook = SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND, HMODULE::default(), Some(winevent), 0, 0, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS);
        a.wake();

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // Clean shutdown: finish pending PNG writes (no half-written file), free everything.
        APP.store(std::ptr::null_mut(), Ordering::Relaxed);
        let _ = UnhookWinEvent(hook);
        for j in a.saving.drain(..) {
            let _ = j.join();
        }
        a.session = None;
        a.gpu = None;
        let _ = DestroyWindow(a.overlay.hwnd);
        let _ = DestroyWindow(ctl);
        wlog!("exit 0");
    }
    std::process::exit(0);
}
