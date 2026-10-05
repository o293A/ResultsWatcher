#![cfg_attr(windows, windows_subsystem = "windows")]
#![allow(non_snake_case)]

#[cfg(windows)]
fn main() {
    results_watcher::win::run();
}

/// Non-Windows builds only offer the offline `--debug-image` mode (used for development/tests).
#[cfg(not(windows))]
fn main() {
    let a: Vec<String> = std::env::args().collect();
    if let Some(i) = a.iter().position(|x| x == "--debug-image") {
        if let Some(p) = a.get(i + 1) {
            print!("{}", results_watcher::debug_image::run(std::path::Path::new(p), None));
            return;
        }
    }
    eprintln!("ResultsWatcher only runs on Windows 10/11 (offline mode: --debug-image <file.png>)");
}
