#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PreviewRequest {
    id: u64,
    epoch: u64,
    intent: u64,
    clock: bool,
    pub(crate) source: String,
    pub(crate) at_ms: u64,
    pub(crate) step: i8,
}

#[derive(Default)]
pub(crate) struct Completion {
    pub(crate) released: bool,
    pub(crate) current: bool,
    pub(crate) publish: bool,
}

struct Clock {
    origin: u64,
    position: u64,
    elapsed: u64,
    awaiting_step: bool,
}

struct Seek {
    at_ms: u64,
    clock: bool,
}

impl Clock {
    fn position(&mut self, now: u64, end: u64) -> u64 {
        self.elapsed = self.elapsed.max(now.saturating_sub(self.origin));
        self.position.saturating_add(self.elapsed).min(end)
    }
}

/// Owns one native request and coalesces seeks/held-arrow input while it is running.
/// The clock follows elapsed time even when decoding cannot display every frame.
#[derive(Default)]
pub(crate) struct Transport {
    epoch: u64,
    serial: u64,
    intent: u64,
    active: Option<PreviewRequest>,
    pending_seek: Option<Seek>,
    pending_step: Option<i8>,
    displayed: Option<u64>,
    position: u64,
    clock: Option<Clock>,
    ended: bool,
}

impl Transport {
    pub(crate) fn reset_source(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.pending_seek = None;
        self.pending_step = None;
        self.displayed = None;
        self.position = 0;
        self.clock = None;
        self.ended = false;
        // The old request still owns the native decoder until its completion arrives.
    }

    pub(crate) fn displayed(&self) -> Option<u64> {
        self.displayed
    }

    pub(crate) fn position(&self) -> u64 {
        self.position
    }

    pub(crate) fn has_pending(&self) -> bool {
        self.pending_seek.is_some() || self.pending_step.is_some()
    }

    pub(crate) fn seek(&mut self, at_ms: u64) {
        self.stop_clock();
        self.intent = self.intent.wrapping_add(1);
        self.ended = false;
        self.queue_seek(at_ms, false);
    }

    fn queue_seek(&mut self, at_ms: u64, clock: bool) {
        self.position = at_ms;
        self.pending_seek = Some(Seek { at_ms, clock });
        self.pending_step = None;
    }

    pub(crate) fn step(&mut self, direction: i8) {
        if matches!(direction, -1 | 1) {
            self.stop_clock();
            self.ended = false;
            if self.pending_seek.as_ref().is_some_and(|seek| seek.clock) {
                self.pending_seek = None;
            }
            if self.displayed.is_some() && self.active.as_ref().is_some_and(|request| request.clock)
            {
                self.intent = self.intent.wrapping_add(1);
            }
            self.pending_step = Some(direction);
        }
    }

    pub(crate) fn play(&mut self, now: u64, position: u64, restart_at: u64, end: u64) -> u64 {
        let awaiting_step = self.pending_step.is_some()
            || self.active.as_ref().is_some_and(|request| {
                request.step != 0 && request.epoch == self.epoch && request.intent == self.intent
            });
        // Keep the requested endpoint even when the covering frame has an earlier PTS.
        // A pending frame action owns the next position and must not be replaced by restart.
        let at_end = self.ended || self.position >= end || position >= end;
        let position = if !awaiting_step && at_end {
            let restart_at = if restart_at <= end { restart_at } else { 0 };
            self.seek(restart_at);
            restart_at
        } else {
            position
        };
        self.ended = false;
        self.position = position;
        self.clock = Some(Clock {
            origin: now,
            position,
            elapsed: 0,
            awaiting_step,
        });
        position
    }

    pub(crate) fn stop_clock(&mut self) {
        self.clock = None;
    }

    pub(crate) fn pause(&mut self, now: u64, end: u64) -> Option<u64> {
        let clock = self.clock.as_mut()?;
        let position = if clock.awaiting_step {
            self.displayed.unwrap_or(self.position)
        } else {
            clock.position(now, end)
        };
        self.seek(position);
        Some(position)
    }

    pub(crate) fn tick(&mut self, now: u64, end: u64) -> Option<(u64, bool)> {
        let clock = self.clock.as_mut()?;
        if clock.awaiting_step {
            return None;
        }
        let position = clock.position(now, end);
        let ended = position >= end;
        self.queue_seek(position, true);
        if ended {
            self.stop_clock();
            self.ended = true;
        }
        Some((position, ended))
    }

    pub(crate) fn take_request(&mut self, source: &str) -> Option<PreviewRequest> {
        if self.active.is_some() {
            return None;
        }
        let (at_ms, step, clock) = if let Some(seek) = self.pending_seek.take() {
            (seek.at_ms, 0, seek.clock)
        } else {
            (
                self.displayed.unwrap_or(self.position),
                self.pending_step.take()?,
                false,
            )
        };
        self.serial = self.serial.wrapping_add(1);
        let request = PreviewRequest {
            id: self.serial,
            epoch: self.epoch,
            intent: self.intent,
            clock,
            source: source.into(),
            at_ms,
            step,
        };
        self.active = Some(request.clone());
        Some(request)
    }

    pub(crate) fn finish(
        &mut self,
        request: &PreviewRequest,
        source: Option<&str>,
        time_ms: Option<u64>,
    ) -> Completion {
        if self.active.as_ref() != Some(request) {
            return Completion::default();
        }
        self.active = None;
        let current = request.epoch == self.epoch
            && request.intent == self.intent
            && source == Some(request.source.as_str());
        // A newer clock tick queues work, but does not invalidate a decoded image.
        // Explicit seeks and source changes do invalidate older completions.
        let publish = current && time_ms.is_some();
        if publish {
            self.displayed = time_ms;
            if request.step != 0 {
                self.position = time_ms.unwrap_or(self.position);
            }
            if request.step != 0
                && self.pending_step.is_none()
                && let Some(clock) = self.clock.as_mut()
                && clock.awaiting_step
            {
                // A frame step's target is unknown until decoding returns its source PTS.
                clock.position = time_ms.unwrap_or(clock.position);
                clock.awaiting_step = false;
            }
        } else if current {
            self.stop_clock();
            self.pending_seek = None;
            self.pending_step = None;
        }
        Completion {
            released: true,
            current,
            publish,
        }
    }
}

pub(crate) const PACKET_LIMIT: usize = 8 * 1024 * 1024 + 24;

pub(crate) struct PreviewPacket<'a> {
    pub(crate) time_ms: u64,
    pub(crate) duration_ms: u64,
    pub(crate) png: &'a [u8],
}

pub(crate) fn parse_packet(bytes: &[u8]) -> Result<PreviewPacket<'_>, String> {
    if bytes.len() < 32 || bytes.len() > PACKET_LIMIT || &bytes[..8] != b"NVSFPRV1" {
        return Err("Invalid or oversized source-frame packet".into());
    }
    let time_ms = u64::from_le_bytes(bytes[8..16].try_into().map_err(|_| "Invalid frame time")?);
    let duration_ms = u64::from_le_bytes(
        bytes[16..24]
            .try_into()
            .map_err(|_| "Invalid frame duration")?,
    );
    if time_ms > 359_999_999_999
        || duration_ms == 0
        || duration_ms > 359_999_999_999
        || !bytes[24..].starts_with(b"\x89PNG\r\n\x1a\n")
    {
        return Err("Invalid source-frame metadata or PNG signature".into());
    }
    Ok(PreviewPacket {
        time_ms,
        duration_ms,
        png: &bytes[24..],
    })
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
