use super::*;

#[test]
fn cleanup_retries_keep_the_first_absolute_deadline() {
    let start = Instant::now();
    let mut stored = None;
    let deadline = cleanup_deadline(&mut stored, start);
    assert_eq!(deadline, start + Duration::from_secs(2));
    // A signal/wait error must not grant a later finish or destructor a new budget.
    for elapsed in [
        Duration::from_millis(1900),
        Duration::from_secs(2),
        Duration::from_secs(3),
    ] {
        assert_eq!(cleanup_deadline(&mut stored, start + elapsed), deadline);
    }
    assert_eq!(
        cleanup_pause(deadline, start).unwrap(),
        Duration::from_millis(10)
    );
    assert_eq!(
        cleanup_pause(deadline, deadline - Duration::from_millis(1)).unwrap(),
        Duration::from_millis(1)
    );
    for now in [deadline, deadline + Duration::from_secs(1)] {
        assert!(
            cleanup_pause(deadline, now)
                .unwrap_err()
                .to_string()
                .contains("cleanup deadline")
        );
    }
}
