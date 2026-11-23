use super::inventory::{ProcessIds, query_with};
use std::{
    cell::{Cell, RefCell},
    io,
    time::{Duration, Instant},
};

#[test]
fn incomplete_job_snapshot_is_requeried_before_any_pid_is_admitted() {
    let started = Instant::now();
    let clock = Cell::new(started);
    let calls = Cell::new(0);
    let delays = RefCell::new(Vec::new());
    let list = query_with(
        started + Duration::from_secs(2),
        |list| {
            calls.set(calls.get() + 1);
            list.assigned = if calls.get() == 1 { 2 } else { 1 };
            list.listed = 1;
            list.ids[0] = if calls.get() == 1 { 41 } else { 73 };
            Ok(16)
        },
        || clock.get(),
        |delay| {
            delays.borrow_mut().push(delay);
            clock.set(clock.get() + delay);
        },
    )
    .unwrap();
    assert_eq!(calls.get(), 2);
    assert_eq!(list.assigned, 1);
    assert_eq!(&list.ids[..list.listed as usize], &[73]);
    assert_eq!(*delays.borrow(), [Duration::from_millis(20)]);
}

#[test]
fn incomplete_empty_job_snapshot_cannot_establish_cleanup() {
    let started = Instant::now();
    let clock = Cell::new(started);
    let calls = Cell::new(0);
    let list = query_with(
        started + Duration::from_secs(2),
        |list| {
            calls.set(calls.get() + 1);
            list.assigned = u32::from(calls.get() == 1);
            list.listed = 0;
            Ok(8)
        },
        || clock.get(),
        |delay| clock.set(clock.get() + delay),
    )
    .unwrap();
    assert_eq!(calls.get(), 2);
    assert_eq!(list.assigned, 0);
    assert_eq!(list.listed, 0);
}

#[test]
fn persistent_incomplete_job_inventory_fails_with_bounded_diagnostics() {
    let started = Instant::now();
    let clock = Cell::new(started);
    let calls = Cell::new(0);
    let error = query_with(
        started + Duration::from_secs(2),
        |list| {
            calls.set(calls.get() + 1);
            list.assigned = 2;
            list.listed = 1;
            list.ids[0] = 41;
            Ok(16)
        },
        || clock.get(),
        |delay| clock.set(clock.get() + delay),
    )
    .err()
    .unwrap();
    assert_eq!(calls.get(), 8);
    assert_eq!(clock.get() - started, Duration::from_millis(140));
    let text = error.to_string();
    for evidence in [
        "incomplete",
        "assigned=2",
        "listed=1",
        "returned_length=16",
        "attempts=8",
        "capacity=4096",
    ] {
        assert!(text.contains(evidence), "missing {evidence}: {text}");
    }
}

#[test]
fn job_inventory_retry_cannot_extend_the_callers_remaining_deadline() {
    for milliseconds in [0, 7, 20, 25] {
        let started = Instant::now();
        let clock = Cell::new(started);
        let calls = Cell::new(0);
        let delays = RefCell::new(Vec::new());
        let result = query_with(
            started + Duration::from_millis(milliseconds),
            |list| {
                calls.set(calls.get() + 1);
                list.assigned = 1;
                list.listed = 0;
                Ok(8)
            },
            || clock.get(),
            |delay| {
                delays.borrow_mut().push(delay);
                clock.set(clock.get() + delay);
            },
        );
        assert!(result.is_err());
        assert_eq!(clock.get() - started, Duration::from_millis(milliseconds));
        assert_eq!(calls.get(), if milliseconds == 25 { 2 } else { 1 });
        assert!(delays.borrow().iter().all(|delay| !delay.is_zero()));
    }
}

#[test]
fn job_inventory_enforces_the_existing_capacity_without_growth() {
    for (assigned, listed) in [(4097, 0), (4096, 4097)] {
        let calls = Cell::new(0);
        let result = query_with(
            Instant::now() + Duration::from_secs(2),
            |list| {
                calls.set(calls.get() + 1);
                list.assigned = assigned;
                list.listed = listed;
                Ok(32_776)
            },
            Instant::now,
            |_| panic!("An overflowing inventory must not be retried."),
        );
        assert!(result.is_err());
        assert_eq!(calls.get(), 1);
        assert!(result.err().unwrap().to_string().contains("capacity=4096"));
    }
    let list = query_with(
        Instant::now() + Duration::from_secs(2),
        |list| {
            list.assigned = 4096;
            list.listed = 4096;
            for (index, id) in list.ids.iter_mut().enumerate() {
                *id = 1000 + index;
            }
            Ok(32_776)
        },
        Instant::now,
        |_| panic!("A complete inventory must not wait."),
    )
    .unwrap();
    assert_eq!(list.listed, 4096);
    assert_eq!(list.ids.len(), 4096);
    assert_eq!(list.ids[0], 1000);
    assert_eq!(list.ids[4095], 5095);
    assert_eq!(std::mem::offset_of!(ProcessIds, ids), 8);
}

#[test]
fn native_job_query_errors_are_never_retried_or_relabelled_as_overflow() {
    for code in [5, 87, 234] {
        let calls = Cell::new(0);
        let result = query_with(
            Instant::now() + Duration::from_secs(2),
            |list| {
                calls.set(calls.get() + 1);
                list.assigned = 2;
                list.listed = 0;
                Err(io::Error::from_raw_os_error(code))
            },
            Instant::now,
            |_| panic!("Native query errors must fail immediately."),
        );
        assert_eq!(result.err().unwrap().raw_os_error(), Some(code));
        assert_eq!(calls.get(), 1);
    }
}
