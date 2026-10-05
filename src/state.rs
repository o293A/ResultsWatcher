//! Pure state machine: SEARCHING -> STABILIZING -> CAPTURING -> CAPTURED -> CLOSING -> SEARCHING.
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
    Stats { banner: Banner, rows: bool, hdr: (u8, u16) },
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
    Stabilizing,
    Capturing,
    Captured,
    Closing,
}

#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// Consecutive identical checks needed (2..3).
    pub stable_checks: u8,
    /// Max wait for the tooltip bubble to settle before capturing anyway.
    pub bubble_wait_ms: u64,
    /// Consecutive absent checks that mean "closed".
    pub close_checks: u8,
    pub max_capture_attempts: u8,
}

impl Default for Params {
    fn default() -> Self {
        Params { stable_checks: 3, bubble_wait_ms: 3000, close_checks: 3, max_capture_attempts: 3 }
    }
}

pub struct Machine {
    p: Params,
    pub state: State,
    stable: u8,
    since_ms: u64,
    last: Option<Obs>,
    missing: u8,
    attempts: u8,
    captured_banner: Option<Banner>,
    banner_diff: u8,
    rows_missing: u8,
    rows_cycle: bool,
}

fn hdr_close(a: (u8, u16), b: (u8, u16)) -> bool {
    (a.0 as i32 - b.0 as i32).abs() <= 2 && (a.1 as i32 - b.1 as i32).abs() <= 6
}

impl Machine {
    pub fn new(p: Params) -> Machine {
        Machine {
            p,
            state: State::Searching,
            stable: 0,
            since_ms: 0,
            last: None,
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
        self.stable = 0;
        self.missing = 0;
        self.attempts = 0;
        self.last = None;
        self.captured_banner = None;
        self.banner_diff = 0;
        self.rows_missing = 0;
        self.rows_cycle = false;
    }

    /// Feed one observation (one detection pass).
    pub fn on_observation(&mut self, obs: Obs, now_ms: u64) -> Action {
        match self.state {
            State::Searching => {
                // Only a clear STATS observation starts anything. Rewards/Other/Absent: do nothing.
                if let Obs::Stats { .. } = obs {
                    self.state = State::Stabilizing;
                    self.stable = 1;
                    self.since_ms = now_ms;
                    self.missing = 0;
                    self.last = Some(obs);
                }
                Action::None
            }
            State::Stabilizing => {
                let (banner, rows, hdr) = match obs {
                    Obs::Stats { banner, rows, hdr } => (banner, rows, hdr),
                    Obs::Other => {
                        // Left the Stats tab before the capture happened: never capture.
                        self.reset_search();
                        return Action::None;
                    }
                    Obs::Absent => {
                        // Tolerate one blip (cursor pass, tooltip): 2 misses in a row -> back.
                        self.missing += 1;
                        if self.missing >= 2 {
                            self.reset_search();
                        }
                        return Action::None;
                    }
                };
                self.missing = 0;
                let same = match self.last {
                    Some(Obs::Stats { banner: pb, hdr: ph, .. }) => pb == banner && hdr_close(ph, hdr),
                    _ => false,
                };
                self.last = Some(obs);
                if same && rows {
                    self.stable += 1;
                } else {
                    self.stable = 1;
                }
                let waited = now_ms.saturating_sub(self.since_ms);
                let settled = self.stable >= self.p.stable_checks;
                // Bubble still animating after the max wait: capture anyway (needs a player row).
                let give_up = waited >= self.p.bubble_wait_ms && rows;
                if settled || give_up {
                    self.state = State::Capturing;
                    self.captured_banner = Some(banner);
                    self.attempts = 1;
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
                Obs::Stats { banner, rows, .. } => {
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
                self.on_observation(obs, now_ms)
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
