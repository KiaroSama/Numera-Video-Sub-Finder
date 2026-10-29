use nvsf_core::close_admission::CloseAdmission;

#[test]
fn rejected_duplicate_does_not_release_the_pending_owner() {
    let admission = CloseAdmission::default();
    let owner = admission.try_begin().expect("first close request admitted");
    assert!(admission.try_begin().is_none());
    assert!(admission.try_begin().is_none());
    drop(owner);
    assert!(admission.try_begin().is_some());
}

#[test]
fn dropping_a_cancelled_pending_request_readmits_a_later_request() {
    let admission = CloseAdmission::default();
    {
        let _pending = admission
            .try_begin()
            .expect("pending close request admitted");
        assert!(admission.try_begin().is_none());
    }
    let retry = admission
        .try_begin()
        .expect("cancelled request released admission");
    assert!(admission.try_begin().is_none());
    drop(retry);
    assert!(admission.try_begin().is_some());
}
