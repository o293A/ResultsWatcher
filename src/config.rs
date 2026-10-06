//! Optional `config.toml` next to the executable. A tiny `key = value` parser is used instead
//! of a TOML crate (smaller binary, no dependency). Unknown keys are ignored; every key has a
//! working default, so no file is needed.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Config {
    /// Output folder; relative paths are resolved next to the executable.
    pub output_dir: String,
    /// Detection passes per second while Roblox is in the foreground.
    pub poll_hz: f64,
    /// Consecutive absent checks that mean "panel closed".
    pub close_checks: u8,
    /// Overlay margin to the right screen edge, in reference pixels (scaled with resolution).
    pub overlay_margin: i32,
    /// Extra overlay size multiplier on top of the resolution-based scale.
    pub overlay_scale: f64,
    /// Esc presses needed / max gap between two presses (ms).
    pub esc_count: u8,
    pub esc_max_gap_ms: u64,
    /// Only honour Esc x3 when Roblox is the foreground window (default: off = always).
    pub esc_only_when_roblox_focused: bool,
    /// Rare full-frame fallback search (multi-scale) when the predicted position fails.
    pub fallback_search: bool,
    pub roblox_process: String,
    /// "window" (default) or "monitor" (Windows Graphics Capture, mouse cursor excluded), or "duplication" (DXGI Desktop Duplication).
    pub capture: String,
    /// Cap of watcher.log in KiB (rotated to watcher.log.1).
    pub log_max_kb: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            output_dir: "screens".into(),
            poll_hz: 3.0,
            close_checks: 3,
            overlay_margin: 24,
            overlay_scale: 1.0,
            esc_count: 3,
            esc_max_gap_ms: 1200,
            esc_only_when_roblox_focused: false,
            fallback_search: true,
            roblox_process: "RobloxPlayerBeta.exe".into(),
            capture: "window".into(),
            log_max_kb: 256,
        }
    }
}

impl Config {
    pub fn parse(text: &str) -> Config {
        let mut c = Config::default();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim().trim_matches('"'));
            match k {
                "output_dir" => c.output_dir = v.to_string(),
                "poll_hz" => c.poll_hz = v.parse().unwrap_or(c.poll_hz).clamp(0.5, 10.0),
                "close_checks" => c.close_checks = v.parse().unwrap_or(c.close_checks).clamp(2, 10),
                "overlay_margin" => c.overlay_margin = v.parse().unwrap_or(c.overlay_margin).clamp(0, 400),
                "overlay_scale" => c.overlay_scale = v.parse().unwrap_or(c.overlay_scale).clamp(0.5, 3.0),
                "esc_count" => c.esc_count = v.parse().unwrap_or(c.esc_count).clamp(2, 10),
                "esc_max_gap_ms" => c.esc_max_gap_ms = v.parse().unwrap_or(c.esc_max_gap_ms).clamp(200, 5000),
                "esc_only_when_roblox_focused" => c.esc_only_when_roblox_focused = v == "true",
                "fallback_search" => c.fallback_search = v != "false",
                "roblox_process" => c.roblox_process = v.to_string(),
                "capture" => c.capture = v.to_ascii_lowercase(),
                "log_max_kb" => c.log_max_kb = v.parse().unwrap_or(c.log_max_kb).clamp(16, 4096),
                _ => {}
            }
        }
        c
    }

    pub fn load(exe_dir: &Path) -> Config {
        match std::fs::read_to_string(exe_dir.join("config.toml")) {
            Ok(t) => Config::parse(&t),
            Err(_) => Config::default(),
        }
    }

    pub fn resolve_output_dir(&self, exe_dir: &Path) -> PathBuf {
        let p = PathBuf::from(&self.output_dir);
        if p.is_absolute() {
            p
        } else {
            exe_dir.join(p)
        }
    }
}
