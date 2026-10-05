//! Small Win32 helpers: process-name test, window geometry, process tuning, console output.

use crate::geometry::{ClientBox, Rect};
use std::ffi::c_void;
use windows::core::PWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub fn set_dpi_awareness() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// Below-normal priority + EcoQoS ("efficiency mode"): the game always wins the CPU.
pub fn tune_process() {
    unsafe {
        let p = GetCurrentProcess();
        let _ = SetPriorityClass(p, BELOW_NORMAL_PRIORITY_CLASS);
        let st = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            StateMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        };
        let _ = SetProcessInformation(
            p,
            ProcessPowerThrottling,
            &st as *const _ as *const c_void,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        );
    }
}

pub fn lower_current_thread() {
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

pub fn exe_name_of_window(hwnd: HWND) -> Option<String> {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 520];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return None;
        }
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        full.rsplit(['\\', '/']).next().map(|s| s.to_string())
    }
}

/// Foreground window if it belongs to the Roblox process, visible and not minimised.
pub fn roblox_foreground(exe: &str) -> Option<HWND> {
    unsafe {
        let h = GetForegroundWindow();
        if h.0.is_null() || IsIconic(h).as_bool() || !IsWindowVisible(h).as_bool() {
            return None;
        }
        match exe_name_of_window(h) {
            Some(n) if n.eq_ignore_ascii_case(exe) => Some(h),
            _ => None,
        }
    }
}

pub struct WinGeom {
    /// Window frame as captured by WGC (screen coordinates).
    pub frame: Rect,
    /// Client area in frame pixels.
    pub client: ClientBox,
    /// Monitor rectangle (screen coordinates) hosting the window.
    pub monitor: Rect,
    pub hmon: HMONITOR,
}

pub fn geometry(hwnd: HWND) -> Option<WinGeom> {
    unsafe {
        let mut fr = RECT::default();
        DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut fr as *mut _ as *mut c_void, std::mem::size_of::<RECT>() as u32).ok()?;
        let mut cr = RECT::default();
        GetClientRect(hwnd, &mut cr).ok()?;
        let mut org = POINT { x: 0, y: 0 };
        if !ClientToScreen(hwnd, &mut org).as_bool() {
            return None;
        }
        let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(hmon, &mut mi).as_bool() {
            return None;
        }
        let m = mi.rcMonitor;
        Some(WinGeom {
            frame: Rect::new(fr.left, fr.top, fr.right - fr.left, fr.bottom - fr.top),
            client: ClientBox { x: org.x - fr.left, y: org.y - fr.top, w: cr.right - cr.left, h: cr.bottom - cr.top },
            monitor: Rect::new(m.left, m.top, m.right - m.left, m.bottom - m.top),
            hmon,
        })
    }
}

/// Print to the parent console (for `--debug-image` in a GUI-subsystem exe).
pub fn console_print(s: &str) {
    use std::io::Write;
    unsafe {
        let _ = windows::Win32::System::Console::AttachConsole(windows::Win32::System::Console::ATTACH_PARENT_PROCESS);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
        let _ = f.write_all(s.as_bytes());
    }
}
