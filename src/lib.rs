//! ResultsWatcher core library.
//!
//! Architecture (see README):
//!  * Pure, cross-platform logic (unit-tested on any OS):
//!    `geometry`, `pixels`, `detect`, `state`, `esc`, `config`, `png_out`, `logger`.
//!  * Windows-only glue (`win` module): Windows Graphics Capture, the layered
//!    overlay window, Raw Input for the Esc x3 shutdown, process tuning.
//!
//! Nothing here injects into, hooks, or reads the memory of the game.

pub mod config;
pub mod debug_image;
pub mod detect;
pub mod esc;
pub mod geometry;
pub mod logger;
pub mod pixels;
pub mod png_out;
pub mod render;
pub mod state;
pub mod template;

#[cfg(windows)]
pub mod win;
