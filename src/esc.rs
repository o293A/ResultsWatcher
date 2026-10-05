//! Esc x3 counter. Pure and event-driven: fed by Raw Input key-down/key-up events.

pub struct EscCounter {
    need: u8,
    max_gap_ms: u64,
    count: u8,
    last_down_ms: u64,
    held: bool,
    locked: bool,
}

impl EscCounter {
    pub fn new(need: u8, max_gap_ms: u64) -> Self {
        EscCounter { need, max_gap_ms, count: 0, last_down_ms: 0, held: false, locked: false }
    }

    /// Returns true exactly once, when the final press of the sequence arrives.
    pub fn on_key(&mut self, down: bool, now_ms: u64) -> bool {
        if self.locked {
            return false; // shutting down: ignore every input
        }
        if !down {
            self.held = false; // release needed between two presses
            return false;
        }
        if self.held {
            return false; // auto-repeat while held: not a new press
        }
        self.held = true;
        if self.count > 0 && now_ms.saturating_sub(self.last_down_ms) > self.max_gap_ms {
            self.count = 0; // too slow: restart
        }
        self.count += 1;
        self.last_down_ms = now_ms;
        if self.count >= self.need {
            self.locked = true;
            return true;
        }
        false
    }

    pub fn is_locked(&self) -> bool {
        self.locked
    }
}
