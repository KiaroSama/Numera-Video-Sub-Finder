use super::IdleWatch;
use std::time::{Duration, Instant};

#[test]
fn resuming_after_a_long_pause_does_not_expire_the_worker_idle_deadline() {
    let started = Instant::now();
    let mut idle = IdleWatch::new(started);
    assert!(!idle.expired(started + Duration::from_secs(100), true));
    assert!(!idle.expired(started + Duration::from_secs(3_600), true));
    assert!(!idle.expired(started + Duration::from_secs(3_601), false));
    assert!(!idle.expired(started + Duration::from_secs(3_801), false));
    assert!(idle.expired(started + Duration::from_secs(3_802), false));
}

#[test]
fn worker_progress_resets_active_idle_time_even_while_pause_is_pending() {
    let started = Instant::now();
    let mut idle = IdleWatch::new(started);
    assert!(!idle.expired(started + Duration::from_secs(300), false));
    assert!(idle.expired(started + Duration::from_secs(301), false));
    idle.progress(started + Duration::from_secs(302));
    assert!(!idle.expired(started + Duration::from_secs(400), true));
    idle.progress(started + Duration::from_secs(450));
    assert!(!idle.expired(started + Duration::from_secs(4_000), false));
    assert!(!idle.expired(started + Duration::from_secs(4_300), false));
    assert!(idle.expired(started + Duration::from_secs(4_301), false));
}
