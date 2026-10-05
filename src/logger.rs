//! Discreet size-capped log (`watcher.log` next to the exe, rotated to `watcher.log.1`).
//! Never shows anything on screen.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

struct Logger {
    path: PathBuf,
    max_bytes: u64,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

pub fn init(path: PathBuf, max_kb: u64) {
    let _ = LOGGER.set(Logger { path, max_bytes: max_kb * 1024 });
}

pub fn log(msg: &str) {
    let Some(l) = LOGGER.get() else { return };
    if let Ok(md) = fs::metadata(&l.path) {
        if md.len() > l.max_bytes {
            let mut old = l.path.clone().into_os_string();
            old.push(".1");
            let _ = fs::rename(&l.path, PathBuf::from(old)); // replaces the previous rotation
        }
    }
    let t = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&l.path) {
        let _ = writeln!(f, "[{}] {}", t, msg);
    }
}

#[macro_export]
macro_rules! wlog {
    ($($arg:tt)*) => { $crate::logger::log(&format!($($arg)*)) };
}
