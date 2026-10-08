//! Windows 后端的窗口串行化与代次消费回归。

use std::sync::Arc;

use super::*;

fn window(process_id: u32, handle: &str, generation: u64) -> ComputerWindow {
    ComputerWindow {
        process_id,
        window_handle: handle.into(),
        title: "fixture".into(),
        generation,
    }
}

#[test]
fn same_window_snapshot_and_action_share_one_operation_lock() {
    let backend = WindowsComputer::default();
    let first = window(7, "42", 1);
    let same = window(7, "42", 1);
    let different = window(7, "43", 1);

    let first_lock = backend.window_lock(&first).unwrap();
    let same_lock = backend.window_lock(&same).unwrap();
    let different_lock = backend.window_lock(&different).unwrap();

    assert!(Arc::ptr_eq(&first_lock, &same_lock));
    assert!(!Arc::ptr_eq(&first_lock, &different_lock));
    let held = first_lock.lock().unwrap();
    assert!(same_lock.try_lock().is_err());
    assert!(different_lock.try_lock().is_ok());
    drop(held);
    assert!(same_lock.try_lock().is_ok());
}

#[test]
fn consuming_generation_before_an_unknown_effect_invalidates_the_old_snapshot() {
    let backend = WindowsComputer::default();
    let old = window(9, "99", 3);
    backend
        .generations
        .lock()
        .unwrap()
        .insert((old.process_id, old.window_handle.clone()), old.generation);

    let next = backend.advance_generation(&old).unwrap();
    let simulated_input_result: Result<(), ComputerError> = Err(ComputerError::UnknownEffect(
        "input acknowledgement was lost".into(),
    ));

    assert!(matches!(
        simulated_input_result,
        Err(ComputerError::UnknownEffect(_))
    ));
    assert!(backend.check_generation(&old).is_err());
    assert!(
        backend
            .check_generation(&window(old.process_id, &old.window_handle, next))
            .is_ok()
    );
}
