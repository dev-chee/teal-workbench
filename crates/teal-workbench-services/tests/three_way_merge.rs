use teal_workbench_services::agent_algorithms::{ThreeWayMerge, ThreeWayMergeError};

#[test]
fn three_way_merge_combines_non_overlapping_and_identical_edits_without_conflicts() {
    let non_overlapping = ThreeWayMerge::new(
        "one\nleft\nright\n",
        "one\nours\nright\n",
        "one\nleft\ntheirs\n",
    );
    assert!(non_overlapping.conflicts.is_empty());
    assert_eq!(non_overlapping.merged, "one\nours\ntheirs\n");

    let identical = ThreeWayMerge::new("one\nvalue\n", "one\nshared\n", "one\nshared\n");
    assert!(identical.conflicts.is_empty());
    assert_eq!(identical.merged, "one\nshared\n");
}

#[test]
fn conflicting_merge_exposes_all_three_inputs_and_validates_manual_resolution() {
    let base = "one\nvalue\nthree\n";
    let ours = "one\nours\nthree\n";
    let theirs = "one\ntheirs\nthree\n";
    let merge = ThreeWayMerge::new(base, ours, theirs);
    assert_eq!(merge.conflicts.len(), 1);
    assert_eq!(merge.conflicts[0].base_start_line, 1);
    assert_eq!(merge.conflicts[0].base_end_line, 2);
    assert_eq!(merge.conflicts[0].base, "value\n");
    assert_eq!(merge.conflicts[0].ours, "ours\n");
    assert_eq!(merge.conflicts[0].theirs, "theirs\n");
    assert!(merge.merged.contains("<<<<<<< ours\nours\n"));
    assert!(merge.merged.contains("||||||| base\nvalue\n"));
    assert!(merge.merged.contains(">>>>>>> theirs\n"));

    let resolution = merge
        .validate_resolution(base, ours, theirs, "one\nresolved\nthree\n")
        .expect("valid manual resolution");
    assert_eq!(resolution.text, "one\nresolved\nthree\n");
    assert_eq!(resolution.input_digest, merge.input_digest);
    assert_eq!(
        merge
            .validate_resolution(base, ours, theirs, "<<<<<<< ours\n")
            .unwrap_err(),
        ThreeWayMergeError::ConflictMarkers
    );
    assert_eq!(
        merge
            .validate_resolution(base, "one\nchanged\nthree\n", theirs, "resolved\n")
            .unwrap_err(),
        ThreeWayMergeError::StaleInputs
    );
}
