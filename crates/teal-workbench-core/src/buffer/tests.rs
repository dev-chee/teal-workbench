use crate::{BufferStore, Document, DocumentChange, TextEdit, Transaction};

#[test]
fn minimal_delta_respects_unicode_and_undo_records_changes() {
    let change = DocumentChange::between("a\u{4e16}😀z", "a\u{754c}😀z", 3, 4);
    assert_eq!(change.old_byte_range, 1..4);
    assert_eq!(change.inserted, "\u{754c}");
    let mut store = BufferStore::default();
    let buffer = store.insert(Document::from_text("a\u{4e16}😀z"));
    buffer
        .apply(
            Transaction::new(vec![TextEdit {
                range: 1..2,
                insert: "\u{754c}".into(),
            }])
            .unwrap(),
        )
        .unwrap();
    assert_eq!(buffer.take_changes().len(), 1);
    buffer.undo().unwrap();
    assert_eq!(buffer.to_string(), "a\u{4e16}😀z");
    assert_eq!(buffer.take_changes()[0].inserted, "\u{4e16}");
}

#[test]
fn handles_share_document_without_borrowing_snapshots() {
    let mut store = BufferStore::default();
    let first = store.insert(Document::from_text("old"));
    let second = store.get(first.id()).unwrap();
    let snapshot = first.text();
    second.apply(Transaction::insert(0, "new ")).unwrap();
    assert_eq!(snapshot.to_string(), "old");
    assert_eq!(first.to_string(), "new old");
}

#[test]
fn view_selections_are_independent_and_io_reservation_rejects_edits() {
    let buffer = crate::BufferHandle::new(Document::from_text("abc"));
    let first = buffer.fork_view();
    let second = buffer.fork_view();
    first.set_selections(crate::SelectionSet::single(crate::Selection::cursor(2)));
    assert_eq!(second.selections().primary().head, 0);
    let snapshot = buffer.begin_io().unwrap();
    assert_eq!(
        first.apply(Transaction::insert(0, "x")),
        Err(crate::TransactionError::BufferBusy)
    );
    buffer.finish_io(snapshot, vec![]);
    second.apply(Transaction::delete(0..3)).unwrap();
    assert_eq!(first.selections().primary().head, 0);
    assert!(first.undo().unwrap());
    assert_eq!(second.to_string(), "abc");
}
#[test]
fn external_line_endings_are_normalized_before_history_journaling_and_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("crlf.txt");
    std::fs::write(&path, b"base\r\n").unwrap();
    let buffer = crate::BufferHandle::new(Document::open(&path).unwrap());
    buffer
        .apply(Transaction::insert(0, "new\r\nnext\r"))
        .unwrap();
    assert_eq!(buffer.to_string(), "new\nnext\nbase\n");
    assert_eq!(
        buffer.take_changes()[0].transaction.edits()[0].insert,
        "new\nnext\n"
    );
    buffer.save().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"new\r\nnext\r\nbase\r\n");
    buffer.undo().unwrap();
    assert_eq!(buffer.to_string(), "base\n");
    let mut document = Document::from_text("base");
    document.apply(Transaction::insert(0, "a\r\n")).unwrap();
    assert_eq!(document.to_string(), "a\nbase");
}
