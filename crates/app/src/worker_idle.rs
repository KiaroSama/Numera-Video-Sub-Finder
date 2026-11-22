use std::time::{Duration, Instant};

pub(super) struct IdleWatch {
    last_progress: Instant,
    paused_since: Option<Instant>,
}

impl IdleWatch {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            last_progress: now,
            paused_since: None,
        }
    }

    pub(super) fn progress(&mut self, now: Instant) {
        self.last_progress = now;
        if self.paused_since.is_some() {
            self.paused_since = Some(now);
        }
    }

    pub(super) fn expired(&mut self, now: Instant, paused: bool) -> bool {
        if paused {
            self.paused_since.get_or_insert(now);
            return false;
        }
        if let Some(since) = self.paused_since.take() {
            self.last_progress += now.saturating_duration_since(since);
        }
        now.saturating_duration_since(self.last_progress) > Duration::from_secs(300)
    }
}

#[cfg(test)]
#[path = "worker_idle_tests.rs"]
mod tests;
