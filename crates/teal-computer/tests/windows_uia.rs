#[cfg(windows)]
#[test]
#[ignore = "requires interactive Windows desktop"]
fn platform_backend_lists_well_formed_windows() {
    let windows = teal_computer::platform_backend().list_windows().unwrap();
    for window in windows {
        assert!(window.process_id > 0);
        assert!(
            window
                .window_handle
                .parse::<isize>()
                .is_ok_and(|handle| handle != 0)
        );
        assert!(window.generation > 0);
    }
}
