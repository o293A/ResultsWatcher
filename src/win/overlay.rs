//! The on-screen confirmation: a borderless, per-pixel-alpha, always-on-top, click-through,
//! never-activating layered window (Discord-in-call style).

use crate::geometry::{overlay_metrics, overlay_rect, OverlayMetrics, Rect};
use crate::pixels::Image;
use crate::render::*;
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

pub enum Content {
    /// "RESULTS FIND / SCREEN" + thumbnail of the saved panel image.
    Found(Image),
    /// "PROGRAM CLOSED" (no thumbnail).
    Closed,
}

pub struct Overlay {
    pub hwnd: HWND,
    mem_dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    size: (i32, i32),
    pos: (i32, i32),
    alpha: f32,
    from: f32,
    to: f32,
    t0: u64,
    dur: u64,
    visible: bool,
}

unsafe extern "system" fn proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match m {
        WM_NCHITTEST => LRESULT(-1),                // HTTRANSPARENT: the click goes to the window below
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize), // never take focus
        _ => DefWindowProcW(h, m, w, l),
    }
}

impl Overlay {
    pub fn new() -> windows::core::Result<Overlay> {
        unsafe {
            let hi = GetModuleHandleW(None)?;
            let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: hi.into(), lpszClassName: w!("RWOverlay"), ..Default::default() };
            RegisterClassW(&wc);
            let ex = WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT;
            let hwnd = CreateWindowExW(ex, w!("RWOverlay"), w!(""), WS_POPUP, 0, 0, 1, 1, HWND::default(), HMENU::default(), hi, None)?;
            // The overlay must never end up in a screenshot (monitor capture sees every window).
            // WDA_EXCLUDEFROMCAPTURE: Windows 10 2004+; silently ignored on older versions.
            let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
            let dc = CreateCompatibleDC(None);
            Ok(Overlay { hwnd, mem_dc: dc, bmp: HBITMAP::default(), old: HGDIOBJ::default(), size: (0, 0), pos: (0, 0), alpha: 0.0, from: 0.0, to: 0.0, t0: 0, dur: 1, visible: false })
        }
    }

    /// Rasterise the content (GDI text mask -> our own compositor) into a premultiplied BGRA canvas.
    fn render(&self, content: &Content, m: &OverlayMetrics) -> Canvas {
        let lines: &[&str] = match content {
            Content::Found(_) => &["RESULTS FIND", "SCREEN"],
            Content::Closed => &["PROGRAM CLOSED"],
        };
        unsafe {
            let dc = CreateCompatibleDC(None);
            let font = CreateFontW(-m.font_px, 0, 0, 0, 600, 0, 0, 0, 1, 0, 0, ANTIALIASED_QUALITY.0 as u32, 0, w!("Segoe UI Semibold"));
            let oldf = SelectObject(dc, font);
            let mut tw = 0;
            for l in lines {
                let u: Vec<u16> = l.encode_utf16().collect();
                let mut sz = SIZE::default();
                let _ = GetTextExtentPoint32W(dc, &u, &mut sz);
                tw = tw.max(sz.cx);
            }
            let thumb_w = if matches!(content, Content::Found(_)) { m.thumb_w } else { 0 };
            let cw = (tw.max(thumb_w) + 2 * m.pad) as usize;
            let mut ch = 2 * m.pad + m.line_h * lines.len() as i32;
            if thumb_w > 0 {
                ch += m.gap + m.thumb_h;
            }
            let (cw, ch) = (cw, ch as usize);
            // GDI text drawn white on black into a top-down 32-bit DIB; its red channel = coverage.
            let bmi = BITMAPINFO { bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: cw as i32, biHeight: -(ch as i32), biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() }, ..Default::default() };
            let mut bits = std::ptr::null_mut();
            let dib = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0).unwrap();
            let oldb = SelectObject(dc, dib);
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, COLORREF(0x00FFFFFF));
            for (i, l) in lines.iter().enumerate() {
                let u: Vec<u16> = l.encode_utf16().collect();
                let mut sz = SIZE::default();
                let _ = GetTextExtentPoint32W(dc, &u, &mut sz);
                let x = (cw as i32 - sz.cx) / 2;
                let _ = TextOutW(dc, x, m.pad + m.line_h * i as i32 + (m.line_h - sz.cy) / 2, &u);
            }
            let raw = std::slice::from_raw_parts(bits as *const u8, cw * ch * 4);
            let mask: Vec<u8> = raw.chunks_exact(4).map(|p| p[2]).collect();
            SelectObject(dc, oldb);
            SelectObject(dc, oldf);
            let _ = DeleteObject(dib);
            let _ = DeleteObject(font);
            let _ = DeleteDC(dc);

            let mut cv = Canvas::new(cw, ch);
            cv.fill_rounded(m.radius as f32, [24, 25, 28], 0.88); // rgba(24,25,28,0.88)
            cv.composite_mask(&mask, [255, 255, 255]);
            if let Content::Found(img) = content {
                let bgra: Vec<u8> = img.rgb.chunks_exact(3).flat_map(|p| [p[2], p[1], p[0], 255]).collect();
                let th = box_downscale_bgra(&bgra, img.w, img.h, m.thumb_w as usize, m.thumb_h as usize);
                let x0 = (cw - th.w) / 2;
                let y0 = (m.pad + m.line_h * 2 + m.gap) as usize;
                cv.blit_thumb(x0, y0.min(ch - th.h), &th, (m.radius as f32 / 3.0).max(2.0), (m.scale as f32).max(1.0));
            }
            cv
        }
    }

    fn set_bitmap(&mut self, cv: &Canvas) {
        unsafe {
            if !self.bmp.is_invalid() {
                SelectObject(self.mem_dc, self.old);
                let _ = DeleteObject(self.bmp);
            }
            let bmi = BITMAPINFO { bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: cv.w as i32, biHeight: -(cv.h as i32), biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() }, ..Default::default() };
            let mut bits = std::ptr::null_mut();
            self.bmp = CreateDIBSection(self.mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0).unwrap();
            std::ptr::copy_nonoverlapping(cv.px.as_ptr(), bits as *mut u8, cv.px.len());
            self.old = SelectObject(self.mem_dc, self.bmp);
            self.size = (cv.w as i32, cv.h as i32);
        }
    }

    fn present(&self) {
        unsafe {
            let pt = POINT { x: self.pos.0, y: self.pos.1 };
            let sz = SIZE { cx: self.size.0, cy: self.size.1 };
            let src = POINT { x: 0, y: 0 };
            let bf = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: (self.alpha * 255.0) as u8, AlphaFormat: AC_SRC_ALPHA as u8 };
            let _ = UpdateLayeredWindow(self.hwnd, HDC::default(), Some(&pt), Some(&sz), self.mem_dc, Some(&src), COLORREF(0), Some(&bf), ULW_ALPHA);
        }
    }

    /// Show on `monitor` (physical px): centred vertically, flush right with `margin`.
    /// `fade_ms` = 0 shows immediately (used for "PROGRAM CLOSED").
    pub fn show(&mut self, content: &Content, monitor: Rect, user_scale: f64, margin_ref: i32, fade_ms: u64, now: u64) {
        let m = overlay_metrics(monitor.h, user_scale, margin_ref);
        let cv = self.render(content, &m);
        self.set_bitmap(&cv);
        let r = overlay_rect(monitor, cv.w as i32, cv.h as i32, m.margin);
        self.pos = (r.x, r.y);
        self.from = if fade_ms == 0 { 1.0 } else { 0.0 };
        self.alpha = self.from;
        self.to = 1.0;
        self.t0 = now;
        self.dur = fade_ms.max(1);
        self.present();
        self.visible = true;
        self.raise();
    }

    pub fn hide(&mut self, fade_ms: u64, now: u64) {
        if !self.visible {
            return;
        }
        self.from = self.alpha;
        self.to = 0.0;
        self.t0 = now;
        self.dur = fade_ms.max(1);
    }

    /// Re-assert TOPMOST without activating (also over fullscreen-windowed games).
    pub fn raise(&self) {
        unsafe {
            let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Advance the fade. Returns true while an animation is running.
    pub fn tick(&mut self, now: u64) -> bool {
        if !self.visible || (self.alpha - self.to).abs() < 1e-3 {
            if self.visible && self.to == 0.0 {
                unsafe {
                    let _ = ShowWindow(self.hwnd, SW_HIDE);
                }
                self.visible = false;
            }
            return false;
        }
        let t = ((now.saturating_sub(self.t0)) as f32 / self.dur as f32).clamp(0.0, 1.0);
        self.alpha = self.from + (self.to - self.from) * t;
        self.present();
        if t >= 1.0 && self.to == 0.0 {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            self.visible = false;
            return false;
        }
        t < 1.0
    }
}
