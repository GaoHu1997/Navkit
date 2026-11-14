use super::*;

#[test]
fn normal_finish_releases_and_clone_drop_does_not() {
    let controller = AdmissionController::new(4, 100);
    let task = controller.try_begin("task", 25).unwrap();
    drop(task.clone());
    assert_eq!(controller.usage(), AdmissionUsage { producers: 1, reserved_bytes: 25, members: 1 });
    assert!(task.finish()); assert!(!task.finish());
    assert_eq!(controller.usage(), AdmissionUsage { producers: 0, reserved_bytes: 0, members: 0 });
}

#[test]
fn cancel_request_does_not_release_until_work_settles() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let controller = AdmissionController::new(1, 10);
    let task = controller.try_begin("working", 10).unwrap();
    let cancel = AtomicBool::new(false); cancel.store(true, Ordering::Release);
    assert!(cancel.load(Ordering::Acquire));
    assert!(matches!(controller.try_begin("next", 1), Err(AdmissionError::Capacity)));
    assert_eq!(controller.usage().reserved_bytes, 10);
    task.finish(); assert!(controller.try_begin("next", 10).unwrap().finish());
}

#[test]
fn duplicate_id_and_stale_finish_cannot_steal_new_ownership() {
    let controller = AdmissionController::new(4, 100);
    let old = controller.try_begin("same", 10).unwrap();
    assert!(matches!(controller.try_begin("same", 10), Err(AdmissionError::DuplicateId)));
    old.finish();
    let new = controller.try_begin("same", 20).unwrap();
    assert!(!old.finish()); assert_eq!(controller.usage().reserved_bytes, 20);
    assert!(new.finish());
}

#[test]
fn count_and_weight_are_independent_and_checked_before_work() {
    let controller = AdmissionController::new(4, 100);
    let a = controller.try_begin("a", 70).unwrap();
    assert!(matches!(controller.try_begin("b", 31), Err(AdmissionError::Capacity)));
    let b = controller.try_begin("b", 30).unwrap();
    assert_eq!(controller.usage().reserved_bytes, 100);
    assert!(matches!(controller.try_begin("overflow", u64::MAX), Err(AdmissionError::Capacity)));
    a.finish(); b.finish();
    let tasks: Vec<_> = (0..4).map(|id| controller.try_begin(&id.to_string(), 1).unwrap()).collect();
    assert!(matches!(controller.try_begin("fifth", 1), Err(AdmissionError::Capacity)));
    for task in tasks { task.finish(); }
}

#[test]
fn nested_at_full_capacity_never_waits_and_last_member_owns_release() {
    let controller = AdmissionController::new(1, 10);
    let parent = controller.try_begin("parent", 10).unwrap();
    let child = parent.nested("child").unwrap();
    let grandchild = child.nested("cursor").unwrap();
    assert_eq!(controller.usage(), AdmissionUsage { producers: 1, reserved_bytes: 10, members: 3 });
    parent.finish(); assert!(matches!(parent.nested("late"), Err(AdmissionError::MissingParent)));
    child.finish(); assert_eq!(controller.usage().reserved_bytes, 10);
    assert!(matches!(controller.try_begin("independent", 1), Err(AdmissionError::Capacity)));
    grandchild.finish(); assert_eq!(controller.usage().reserved_bytes, 0);
}

#[test]
fn nested_requires_live_parent_in_same_controller() {
    let one = AdmissionController::new(1, 10); let two = AdmissionController::new(1, 10);
    let parent = one.try_begin("parent", 10).unwrap();
    assert!(matches!(two.try_nested("child", &parent), Err(AdmissionError::ForeignController)));
    assert!(matches!(parent.nested("parent"), Err(AdmissionError::DuplicateId)));
    parent.finish(); assert!(matches!(parent.nested("child"), Err(AdmissionError::MissingParent)));
}

#[test]
fn unfinished_drop_fails_closed_even_when_no_handle_remains() {
    let controller = AdmissionController::new(1, 10);
    drop(controller.try_begin("detached", 10).unwrap());
    assert_eq!(controller.usage().reserved_bytes, 10);
    assert!(matches!(controller.try_begin("next", 1), Err(AdmissionError::Capacity)));
}

#[test]
fn panic_cleanup_releases_only_after_background_work_has_joined() {
    use std::sync::mpsc;
    let controller = AdmissionController::new(1, 10);
    let task = controller.try_begin("panic", 10).unwrap();
    let (started_tx, started_rx) = mpsc::channel(); let (settle_tx, settle_rx) = mpsc::channel();
    let worker_task = task.clone();
    let worker = std::thread::spawn(move || {
        started_tx.send(()).unwrap(); settle_rx.recv().unwrap();
        drop(worker_task); panic!("simulated worker error after cleanup");
    });
    started_rx.recv().unwrap();
    assert_eq!(controller.usage().reserved_bytes, 10);
    assert!(matches!(controller.try_begin("next", 1), Err(AdmissionError::Capacity)));
    settle_tx.send(()).unwrap(); assert!(worker.join().is_err());
    // Actual work has now settled; owner's error path explicitly finishes it.
    assert_eq!(controller.usage().reserved_bytes, 10);
    task.finish(); assert_eq!(controller.usage().reserved_bytes, 0);
}

#[test]
fn default_profile_is_finite_and_overflow_is_rejected() {
    assert_eq!(TransferAdmissionProfile::default().reserved_bytes, 205 * MIB);
    assert!(matches!(TransferAdmissionProfile::bounded(u64::MAX, 1, 1), Err(AdmissionError::InvalidBudget)));
    assert!(matches!(TransferAdmissionProfile::bounded(1, 0, 1), Err(AdmissionError::InvalidBudget)));
}
