#[cfg(loom)]
use loom::sync::atomic::{AtomicBool, Ordering};
#[cfg(not(loom))]
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
pub struct CloseAdmission {
    pending: AtomicBool,
}

impl CloseAdmission {
    pub fn try_begin(&self) -> Option<PendingClose<'_>> {
        // Rejected requests must not add a write to the admission flag.
        if self
            .pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            Some(PendingClose { admission: self })
        } else {
            None
        }
    }
}

#[must_use = "hold the lease until the pending close request finishes or is cancelled"]
pub struct PendingClose<'a> {
    admission: &'a CloseAdmission,
}

impl Drop for PendingClose<'_> {
    fn drop(&mut self) {
        self.admission.pending.store(false, Ordering::Release);
    }
}
