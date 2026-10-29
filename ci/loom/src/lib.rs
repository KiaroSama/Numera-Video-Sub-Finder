#[cfg(all(test, not(loom)))]
compile_error!(
    "Run this harness with RUSTFLAGS=--cfg=loom; std-only execution is not model evidence"
);

#[cfg(all(test, loom))]
#[path = "../../../crates/core/src/close_admission.rs"]
mod close_admission;

#[cfg(all(test, loom))]
mod tests {
    use super::close_admission::CloseAdmission;
    use loom::sync::Arc;
    use loom::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn actual_close_lease_excludes_duplicates_and_reopens_after_drop() {
        let mut model = loom::model::Builder::new();
        model.max_threads = 3;
        model.max_branches = 1000;
        // Inherited limits/checkpoints must not admit partial or empty exploration.
        model.max_permutations = None;
        model.max_duration = None;
        model.preemption_bound = None;
        model.checkpoint_file = None;
        model.checkpoint_interval = 20_000;
        let executions = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = std::sync::Arc::clone(&executions);
        model.check(move || {
            // Observation only; production synchronization uses Loom atomics below.
            observed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let admission = Arc::new(CloseAdmission::default());
            let active = Arc::new(AtomicUsize::new(0));
            let mut threads = Vec::new();
            for _ in 0..2 {
                let admission = Arc::clone(&admission);
                let active = Arc::clone(&active);
                threads.push(loom::thread::spawn(move || {
                    if let Some(pending) = admission.try_begin() {
                        assert_eq!(active.fetch_add(1, Ordering::SeqCst), 0);
                        assert!(admission.try_begin().is_none());
                        loom::thread::yield_now();
                        assert_eq!(active.fetch_sub(1, Ordering::SeqCst), 1);
                        drop(pending);
                    }
                }));
            }
            for thread in threads {
                thread.join().expect("model thread panicked");
            }
            assert_eq!(active.load(Ordering::SeqCst), 0);
            let pending = admission
                .try_begin()
                .expect("dropped close remains pending");
            assert!(admission.try_begin().is_none());
            drop(pending);
            assert!(admission.try_begin().is_some());
        });
        let executions = executions.load(std::sync::atomic::Ordering::Relaxed);
        assert!(executions > 1, "model did not explore multiple executions");
        println!("\nclose_admission_executions={executions}");
    }
}
