//! Pure state machine: SEARCHING -> CAPTURING -> CAPTURED -> CLOSING -> SEARCHING.
//! One capture per panel opening. Time is injected (ms) so it is fully unit-testable.

use crate::detect::Banner;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Obs {
    /// No panel at all (header not found either): the panel is closed or hidden.
    Absent,
    /// Panel still open but the Stats tab is NOT active (e.g. user is on Rewards). It is only
    /// used to know the panel is still there; it can never cause a capture.
    Other,
    /// Panel open with the Stats tab active.
    Stats { banner: Banner, rows: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    /// Capture the panel rectangle now, then call `capture_result`.
    Capture,
    /// Panel closed: fade the overlay out.
    HideOverlay,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Searching,
    Capturing,
    Captured,
    Closing,
}

#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// Consecutive absent checks that mean "closed".
    pub close_checks: u8,
    pub max_capture_attempts: u8,
}

impl Default for Params {
    fn default() -> Self {
        Params { close_checks: 3, max_capture_attempts: 3 }
    }
}

pub struct Machine {
    p: Params,
    pub state: State,
    missing: u8,
    attempts: u8,
    captured_banner: Option<Banner>,
    banner_diff: u8,
    rows_missing: u8,
    rows_cycle: bool,
}

impl Machine {
    pub fn new(p: Params) -> Machine {
        Machine {
            p,
            state: State::Searching,
            missing: 0,
            attempts: 0,
            captured_banner: None,
            banner_diff: 0,
            rows_missing: 0,
            rows_cycle: false,
        }
    }

    fn reset_search(&mut self) {
        self.state = State::Searching;
        self.missing = 0;
        self.attempts = 0;
        self.captured_banner = None;
        self.banner_diff = 0;
        self.rows_missing = 0;
        self.rows_cycle = false;
    }

    /// Feed one observation (one detection pass).
    pub fn on_observation(&mut self, obs: Obs, _now_ms: u64) -> Action {
        match self.state {
            State::Searching => {
                // The first clear STATS observation with a player row captures immediately.
                // Rewards/Other/Absent (or no row yet): do nothing.
                if let Obs::Stats { banner, rows: true } = obs {
                    self.state = State::Capturing;
                    self.captured_banner = Some(banner);
                    self.attempts = 1;
                    self.missing = 0;
                    return Action::Capture;
                }
                Action::None
            }
            State::Capturing => {
                // Waiting for `capture_result`; if the panel vanished meanwhile, give up.
                match obs {
                    Obs::Absent => {
                        self.missing += 1;
                        if self.missing >= self.p.close_checks {
                            self.reset_search();
                        }
                    }
                    _ => self.missing = 0,
                }
                Action::None
            }
            State::Captured => match obs {
                Obs::Absent => {
                    self.missing += 1;
                    if self.missing >= self.p.close_checks {
                        self.state = State::Closing;
                        return Action::HideOverlay;
                    }
                    Action::None
                }
                // User switched to another tab: panel still open, keep everything, no capture.
                Obs::Other => {
                    self.missing = 0;
                    Action::None
                }
                Obs::Stats { banner, rows } => {
                    self.missing = 0;
                    // New game while the panel never closed: banner colour flipped (debounced)...
                    if let Some(cb) = self.captured_banner {
                        if banner != cb {
                            self.banner_diff += 1;
                            if self.banner_diff >= 3 {
                                self.reset_search();
                                return Action::None;
                            }
                        } else {
                            self.banner_diff = 0;
                        }
                    }
                    // ... or the table was rebuilt (rows vanished >= 3 checks, then came back).
                    if !rows {
                        self.rows_missing = self.rows_missing.saturating_add(1);
                        if self.rows_missing >= 3 {
                            self.rows_cycle = true;
                        }
                    } else {
                        if self.rows_cycle {
                            self.reset_search();
                            return Action::None;
                        }
                        self.rows_missing = 0;
                    }
                    Action::None
                }
            },
            State::Closing => {
                self.reset_search();
                self.on_observation(obs, _now_ms)
            }
        }
    }

    /// Report the outcome of the capture requested by `Action::Capture`.
    pub fn capture_result(&mut self, ok: bool) -> Action {
        if self.state != State::Capturing {
            return Action::None;
        }
        if ok {
            self.state = State::Captured;
            self.missing = 0;
            return Action::None;
        }
        if self.attempts < self.p.max_capture_attempts {
            self.attempts += 1;
            return Action::Capture; // immediate retry
        }
        // Give up on this opening: do not loop forever on a panel that cannot be read.
        self.state = State::Captured;
        Action::None
    }
}
