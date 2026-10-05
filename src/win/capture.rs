//! Screen capture + D3D11. Only tiny GPU->CPU copies for detection; the whole panel
//! is read exactly once, at the final capture. The cursor is never captured.
//!
//! Two backends:
//!  * `Source::Duplication` (default): DXGI Desktop Duplication on the monitor. Windows never
//!    draws a yellow capture border with this API, on any Windows version / PC.
//!  * `Source::Window` / `Source::Monitor`: Windows Graphics Capture (WGC). Windows draws a
//!    yellow border around what is captured unless it is Win11 and the border can be disabled.

use crate::geometry::Rect;
use crate::pixels::{Image, Sampler};
use windows::core::Interface;
use windows::Foundation::TimeSpan;
use windows::Graphics::Capture::*;
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{E_FAIL, HWND};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::WinRT::Direct3D11::*;
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;

pub struct Gpu {
    pub dev: ID3D11Device,
    pub ctx: ID3D11DeviceContext,
    pub winrt_dev: IDirect3DDevice,
    /// Monitor this device was created for (Desktop Duplication needs the GPU that drives it).
    pub hmon: Option<HMONITOR>,
}

/// Finds the DXGI adapter + output that drive `hmon`.
fn find_output(hmon: HMONITOR) -> Option<(IDXGIAdapter1, IDXGIOutput)> {
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            let mut j = 0;
            while let Ok(out) = adapter.EnumOutputs(j) {
                if let Ok(d) = out.GetDesc() {
                    if d.Monitor == hmon {
                        return Some((adapter, out));
                    }
                }
                j += 1;
            }
            i += 1;
        }
        None
    }
}

impl Gpu {
    /// `hmon = Some(..)`: create the device on the adapter that drives that monitor (Desktop
    /// Duplication). `None`: default hardware adapter (WGC).
    pub fn new(hmon: Option<HMONITOR>) -> windows::core::Result<Gpu> {
        unsafe {
            let (mut dev, mut ctx) = (None, None);
            let adapter: Option<IDXGIAdapter> = hmon.and_then(find_output).and_then(|(a, _)| a.cast().ok());
            match &adapter {
                Some(a) => D3D11CreateDevice(a, D3D_DRIVER_TYPE_UNKNOWN, None, D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut dev), None, Some(&mut ctx))?,
                None => D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, None, D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut dev), None, Some(&mut ctx))?,
            }
            let dev = dev.unwrap();
            let dxgi: IDXGIDevice = dev.cast()?;
            let _ = dxgi.SetGPUThreadPriority(-7); // lowest: never compete with the game
            let winrt_dev: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?.cast()?;
            Ok(Gpu { dev, ctx: ctx.unwrap(), winrt_dev, hmon })
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    /// DXGI Desktop Duplication on the monitor: NO yellow border, ever.
    Duplication,
    /// Windows Graphics Capture on the window (yellow border possible).
    Window,
    /// Windows Graphics Capture on the monitor (yellow border possible).
    Monitor,
}

enum Backend {
    Wgc { item: GraphicsCaptureItem, pool: Direct3D11CaptureFramePool, sess: GraphicsCaptureSession },
    Dup { dupl: IDXGIOutputDuplication, copy: Option<ID3D11Texture2D>, has_frame: bool },
}

/// One live capture session. Dropping it stops the capture (frees GPU work while the game is not active).
pub struct Session {
    backend: Backend,
    pub size: (i32, i32),
    lost: bool,
}

impl Session {
    pub fn start(gpu: &Gpu, source: Source, hwnd: HWND, hmon: HMONITOR) -> windows::core::Result<Session> {
        if source == Source::Duplication {
            return Self::start_duplication(gpu, hmon);
        }
        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item: GraphicsCaptureItem = unsafe {
            match source {
                Source::Monitor => interop.CreateForMonitor(hmon)?,
                _ => interop.CreateForWindow(hwnd)?,
            }
        };
        let sz = item.Size()?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&gpu.winrt_dev, DirectXPixelFormat::B8G8R8A8UIntNormalized, 3, sz)?;
        let sess = pool.CreateCaptureSession(&item)?;
        let _ = sess.SetIsCursorCaptureEnabled(false); // never capture the mouse cursor
        let _ = sess.SetIsBorderRequired(false); // Win11 only; ignored when unavailable
        let _ = sess.SetMinUpdateInterval(TimeSpan { Duration: 3_000_000 }); // >= 300 ms between frames
        sess.StartCapture()?;
        Ok(Session { backend: Backend::Wgc { item, pool, sess }, size: (sz.Width, sz.Height), lost: false })
    }

    /// Desktop Duplication of the monitor `hmon`. No capture border, on any Windows version.
    /// The mouse cursor is not part of the duplicated image (it is delivered separately).
    fn start_duplication(gpu: &Gpu, hmon: HMONITOR) -> windows::core::Result<Session> {
        let (_adapter, out) = find_output(hmon).ok_or_else(|| windows::core::Error::from(E_FAIL))?;
        let out1: IDXGIOutput1 = out.cast()?;
        let dupl = unsafe { out1.DuplicateOutput(&gpu.dev)? };
        let d = unsafe { dupl.GetDesc() };
        let size = (d.ModeDesc.Width as i32, d.ModeDesc.Height as i32);
        Ok(Session { backend: Backend::Dup { dupl, copy: None, has_frame: false }, size, lost: false })
    }

    /// True when the capture must be rebuilt (access lost: resolution change, UAC, mode switch...).
    pub fn lost(&self) -> bool {
        self.lost
    }

    /// Latest frame (older queued frames are dropped). `None` if no new frame.
    pub fn latest(&mut self, gpu: &Gpu) -> Option<Frame> {
        if matches!(self.backend, Backend::Dup { .. }) {
            return self.latest_dup(gpu);
        }
        let Backend::Wgc { pool, .. } = &self.backend else { return None };
        let mut last = None;
        while let Ok(f) = pool.TryGetNextFrame() {
            last = Some(f);
        }
        let f = last?;
        let cs = f.ContentSize().ok()?;
        if (cs.Width, cs.Height) != self.size {
            // Window resized: rebuild buffers for the new size, use the next frame.
            self.size = (cs.Width, cs.Height);
            let _ = pool.Recreate(&gpu.winrt_dev, DirectXPixelFormat::B8G8R8A8UIntNormalized, 3, SizeInt32 { Width: cs.Width, Height: cs.Height });
            return None;
        }
        let surf = f.Surface().ok()?;
        let acc: IDirect3DDxgiInterfaceAccess = surf.cast().ok()?;
        let tex: ID3D11Texture2D = unsafe { acc.GetInterface().ok()? };
        Some(Frame { _f: Some(f), tex, w: cs.Width, h: cs.Height })
    }

    fn latest_dup(&mut self, gpu: &Gpu) -> Option<Frame> {
        let (w, h) = self.size;
        let Backend::Dup { dupl, copy, has_frame } = &mut self.backend else { return None };
        unsafe {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut res: Option<IDXGIResource> = None;
            match dupl.AcquireNextFrame(0, &mut info, &mut res) {
                Ok(()) => {
                    // LastPresentTime == 0: only the mouse moved, the image did not change.
                    let fresh = info.LastPresentTime != 0 || !*has_frame;
                    let mut ok = false;
                    if fresh {
                        if let Some(tex) = res.and_then(|r| r.cast::<ID3D11Texture2D>().ok()) {
                            if copy.is_none() {
                                // Own persistent copy: the duplication frame must be released right away.
                                let mut d = D3D11_TEXTURE2D_DESC::default();
                                tex.GetDesc(&mut d);
                                d.Usage = D3D11_USAGE_DEFAULT;
                                d.BindFlags = 0;
                                d.CPUAccessFlags = 0;
                                d.MiscFlags = 0;
                                d.MipLevels = 1;
                                d.ArraySize = 1;
                                let mut t = None;
                                if gpu.dev.CreateTexture2D(&d, None, Some(&mut t)).is_ok() {
                                    *copy = t;
                                }
                            }
                            if let Some(c) = copy.as_ref() {
                                if let (Ok(dst), Ok(src)) = (c.cast::<ID3D11Resource>(), tex.cast::<ID3D11Resource>()) {
                                    gpu.ctx.CopyResource(&dst, &src);
                                    *has_frame = true;
                                    ok = true;
                                }
                            }
                        }
                    }
                    let _ = dupl.ReleaseFrame();
                    if ok {
                        copy.clone().map(|t| Frame { _f: None, tex: t, w, h })
                    } else {
                        None
                    }
                }
                Err(e) => {
                    if e.code() != DXGI_ERROR_WAIT_TIMEOUT {
                        self.lost = true; // DXGI_ERROR_ACCESS_LOST etc.: rebuild the session
                    }
                    None
                }
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Backend::Wgc { item, pool, sess } = &self.backend {
            let _ = sess.Close();
            let _ = pool.Close();
            let _ = item;
        }
        // Duplication: dropping IDXGIOutputDuplication releases it.
    }
}

pub struct Frame {
    _f: Option<Direct3D11CaptureFrame>,
    pub tex: ID3D11Texture2D,
    pub w: i32,
    pub h: i32,
}

fn staging(gpu: &Gpu, w: u32, h: u32) -> windows::core::Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut t = None;
    unsafe { gpu.dev.CreateTexture2D(&desc, None, Some(&mut t))? };
    Ok(t.unwrap())
}

fn copy_region(gpu: &Gpu, dst: &ID3D11Resource, dx: u32, dy: u32, src: &ID3D11Resource, r: Rect) {
    let b = D3D11_BOX { left: r.x as u32, top: r.y as u32, front: 0, right: (r.x + r.w) as u32, bottom: (r.y + r.h) as u32, back: 1 };
    unsafe { gpu.ctx.CopySubresourceRegion(dst, 0, dx, dy, 0, src, 0, Some(&b)) };
}

/// Copy the top-left `w` x `h` pixels of a mapped staging texture into a tight BGRA buffer.
fn read_back(gpu: &Gpu, tex: &ID3D11Resource, w: usize, h: usize) -> Option<Vec<u8>> {
    unsafe {
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        gpu.ctx.Map(tex, 0, D3D11_MAP_READ, 0, Some(&mut m)).ok()?;
        let mut out = vec![0u8; w * h * 4];
        for y in 0..h {
            let row = std::slice::from_raw_parts((m.pData as *const u8).add(y * m.RowPitch as usize), w * 4);
            out[y * w * 4..(y + 1) * w * 4].copy_from_slice(row);
        }
        gpu.ctx.Unmap(tex, 0);
        Some(out)
    }
}

/// Atlas sampler: several tiny frame rectangles copied side by side into ONE small staging
/// texture and read back with a single Map. Pixels outside the fetched rectangles return `None`.
pub struct Atlas {
    items: Vec<(Rect, i32, i32)>, // frame rect (clamped), atlas x, atlas y
    data: Vec<u8>,
    w: usize,
}

const ATLAS_W: u32 = 1024;
const ATLAS_H: u32 = 160;

impl Atlas {
    pub fn fetch(gpu: &Gpu, cache: &mut Option<ID3D11Texture2D>, frame: &Frame, rects: &[Rect]) -> Option<Atlas> {
        if cache.is_none() {
            *cache = Some(staging(gpu, ATLAS_W, ATLAS_H).ok()?);
        }
        let st: ID3D11Resource = cache.as_ref()?.cast().ok()?;
        let src: ID3D11Resource = frame.tex.cast().ok()?;
        let (mut x, mut y, mut row_h) = (0i32, 0i32, 0i32);
        let mut items = Vec::with_capacity(rects.len());
        for r in rects {
            let c = Rect::new(r.x.max(0), r.y.max(0), 0, 0);
            let right = (r.x + r.w).min(frame.w);
            let bottom = (r.y + r.h).min(frame.h);
            let c = Rect::new(c.x, c.y, right - c.x, bottom - c.y);
            if c.w <= 0 || c.h <= 0 {
                continue;
            }
            if x + c.w > ATLAS_W as i32 {
                x = 0;
                y += row_h;
                row_h = 0;
            }
            if y + c.h > ATLAS_H as i32 || c.w > ATLAS_W as i32 {
                return None;
            }
            copy_region(gpu, &st, x as u32, y as u32, &src, c);
            items.push((c, x, y));
            x += c.w;
            row_h = row_h.max(c.h);
        }
        let used_h = (y + row_h).max(1) as usize;
        let data = read_back(gpu, &st, ATLAS_W as usize, used_h)?; // only the rows actually used
        Some(Atlas { items, data, w: ATLAS_W as usize })
    }
}

impl Sampler for Atlas {
    fn get(&self, x: i32, y: i32) -> Option<[u8; 3]> {
        for (r, ax, ay) in &self.items {
            if r.contains(x, y) {
                let i = ((*ay + y - r.y) as usize * self.w + (*ax + x - r.x) as usize) * 4;
                return Some([self.data[i + 2], self.data[i + 1], self.data[i]]);
            }
        }
        None
    }
}

/// Read ONE rectangle (the panel) as RGB. Single full-resolution read of the whole program.
pub fn read_rect_rgb(gpu: &Gpu, frame: &Frame, r: Rect) -> Option<Image> {
    if !r.inside(frame.w, frame.h) {
        return None;
    }
    let st = staging(gpu, r.w as u32, r.h as u32).ok()?;
    let dst: ID3D11Resource = st.cast().ok()?;
    let src: ID3D11Resource = frame.tex.cast().ok()?;
    copy_region(gpu, &dst, 0, 0, &src, r);
    let px = read_back(gpu, &dst, r.w as usize, r.h as usize)?;
    Some(Image::from_bgra(r.w as usize, r.h as usize, r.w as usize * 4, &px))
}

/// Full frame read (rare fallback search only; never written to disk).
pub fn read_full(gpu: &Gpu, frame: &Frame) -> Option<Image> {
    read_rect_rgb(gpu, frame, Rect::new(0, 0, frame.w, frame.h))
}
