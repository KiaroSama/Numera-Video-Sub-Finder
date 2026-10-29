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
        if self.pending.swap(true, Ordering::AcqRel) {
            None
        } else {
            Some(PendingClose { admission: self })
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
