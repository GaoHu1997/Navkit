use super::*;

fn isolated_task(controller: &Arc<transfer_admission::AdmissionController>, id: &str, bytes: u64) -> TaskControl {
    TaskControl::register(id, || Ok(Some(controller.try_begin(id, bytes)?))).unwrap()
}

#[test]
fn cancellation_and_cloned_task_control_drop_keep_budget_until_finish() {
    let controller = transfer_admission::AdmissionController::new(1, 10);
    let id = format!("admission_cancel_{}", uuid::Uuid::new_v4().simple());
    let task = isolated_task(&controller, &id, 10);
    drop(task.clone()); cancel_task(&id).unwrap();
    assert!(task.check().is_err()); assert_eq!(controller.usage().reserved_bytes, 10);
    assert!(controller.try_begin("next", 1).is_err());
    task.finish(&Err::<(), _>(anyhow!("work has joined and cleanup completed")));
    assert_eq!(controller.usage().reserved_bytes, 0);
    assert_eq!(task_progress(&id).unwrap().status, "cancelled");
}

#[test]
fn task_control_nested_finish_and_unfinished_drop_are_fail_closed() {
    let controller = transfer_admission::AdmissionController::new(1, 10);
    let id = format!("admission_parent_{}", uuid::Uuid::new_v4().simple());
    let child_id = format!("admission_child_{}", uuid::Uuid::new_v4().simple());
    let parent = isolated_task(&controller, &id, 10);
    let child = TaskControl::begin_nested(&child_id, &parent).unwrap();
    parent.finish(&Ok(())); drop(parent);
    assert_eq!(controller.usage().reserved_bytes, 10);
    drop(child.clone()); assert_eq!(controller.usage().reserved_bytes, 10);
    child.finish(&Ok(())); assert_eq!(controller.usage().reserved_bytes, 0);
    let unfinished_id = format!("admission_drop_{}", uuid::Uuid::new_v4().simple());
    let unfinished = isolated_task(&controller, &unfinished_id, 10);
    let worker = unfinished.clone(); drop(unfinished);
    assert_eq!(controller.usage().reserved_bytes, 10);
    worker.finish(&Err::<(), _>(anyhow!("worker error cleaned up")));
    assert_eq!(controller.usage().reserved_bytes, 0);
}

#[test]
fn stale_task_control_cannot_finish_or_update_reused_id() {
    let controller = transfer_admission::AdmissionController::new(1, 20);
    let id = format!("admission_reuse_{}", uuid::Uuid::new_v4().simple());
    let old = isolated_task(&controller, &id, 10);
    old.finish(&Ok(()));
    // Simulate existing terminal-log pruning without touching other test entries.
    tasks().lock().remove(&id);
    let new = isolated_task(&controller, &id, 20);
    old.finish(&Ok(())); old.update("stale", 99, 99, "must not overwrite new owner");
    assert_eq!(controller.usage().reserved_bytes, 20);
    assert_eq!(task_progress(&id).unwrap().status, "running");
    assert_eq!(task_progress(&id).unwrap().phase, "validate");
    new.finish(&Ok(())); assert_eq!(controller.usage().reserved_bytes, 0);
}
