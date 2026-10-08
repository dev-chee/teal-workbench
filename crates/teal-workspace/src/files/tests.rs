//! 实际文件上的版本冲突、编码保留、撤销和日志核对测试。

use super::*;

/// 测试独占的临时目录，删除前核对父目录与固定前缀。
struct Fixture(PathBuf);
impl Fixture {
    /// 建立与用户文件无关的工作区。
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("teal-workspace-test-{}", change_id()));
        fs::create_dir(&root).unwrap();
        Self(root.canonicalize().unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let base = std::env::temp_dir().canonicalize().unwrap();
        assert_eq!(self.0.parent(), Some(base.as_path()));
        assert!(
            self.0
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("teal-workspace-test-")
        );
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn stale_snapshot_and_later_user_edit_are_not_overwritten() {
    let fixture = Fixture::new();
    let path = fixture.0.join("main.tl");
    fs::write(&path, "original").unwrap();
    let workspace = Workspace::open(&fixture.0).unwrap();
    let old = workspace.read(&path).unwrap();
    fs::write(&path, "user edit").unwrap();
    assert!(matches!(
        workspace.replace(&old, "agent"),
        Err(WorkspaceError::Conflict(_))
    ));
    let current = workspace.read(&path).unwrap();
    let record = workspace.replace(&current, "managed").unwrap();
    fs::write(&path, "later user edit").unwrap();
    assert!(matches!(
        workspace.undo(&record.id),
        Err(WorkspaceError::Conflict(_))
    ));
    assert_eq!(fs::read_to_string(path).unwrap(), "later user edit");
}

#[test]
fn utf16_and_crlf_survive_edit_and_exact_undo() {
    let fixture = Fixture::new();
    let path = fixture.0.join("\u{4e2d}\u{6587}.tl");
    let original = encode(
        "\u{539f}\u{6587}\r\n\u{7b2c}\u{4e8c}\u{884c}\r\n",
        TextEncoding::Utf16Le,
    );
    fs::write(&path, &original).unwrap();
    let workspace = Workspace::open(&fixture.0).unwrap();
    let snapshot = workspace.read(&path).unwrap();
    let record = workspace
        .replace(&snapshot, "\u{4fee}\u{6539}\n\u{7b2c}\u{4e8c}\u{884c}\n")
        .unwrap();
    let updated = workspace.read(&path).unwrap();
    assert_eq!(updated.encoding, TextEncoding::Utf16Le);
    assert_eq!(
        updated.text,
        "\u{4fee}\u{6539}\r\n\u{7b2c}\u{4e8c}\u{884c}\r\n"
    );
    assert_eq!(
        workspace.undo(&record.id).unwrap().state,
        ChangeState::Undone
    );
    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn prepared_record_can_be_reconciled_after_reopening() {
    let fixture = Fixture::new();
    let path = fixture.0.join("file");
    fs::write(&path, "before").unwrap();
    let workspace = Workspace::open(&fixture.0).unwrap();
    let mut record = workspace
        .replace(&workspace.read(&path).unwrap(), "after")
        .unwrap();
    record.state = ChangeState::Prepared;
    workspace.save(&record).unwrap();
    let reopened = Workspace::open(&fixture.0).unwrap();
    assert_eq!(
        reopened.inspect(&record.id).unwrap().state,
        ChangeState::Applied
    );
    assert!(reopened.inspect("../../outside").is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), "after");
}

#[test]
fn journal_cannot_target_outside_workspace() {
    let fixture = Fixture::new();
    let path = fixture.0.join("file");
    fs::write(&path, "before").unwrap();
    let workspace = Workspace::open(&fixture.0).unwrap();
    let mut record = workspace
        .replace(&workspace.read(&path).unwrap(), "after")
        .unwrap();
    record.path = PathBuf::from("../outside");
    workspace.save(&record).unwrap();
    assert!(matches!(
        workspace.inspect(&record.id),
        Err(WorkspaceError::InvalidJournal(_))
    ));
}

#[test]
fn interrupted_undo_is_reconciled_without_overwriting_again() {
    let fixture = Fixture::new();
    let path = fixture.0.join("file");
    fs::write(&path, "before").unwrap();
    let workspace = Workspace::open(&fixture.0).unwrap();
    let mut record = workspace
        .replace(&workspace.read(&path).unwrap(), "after")
        .unwrap();
    record.state = ChangeState::UndoPrepared;
    workspace.save(&record).unwrap();
    fs::write(&path, "before").unwrap();
    let reopened = Workspace::open(&fixture.0).unwrap();
    assert_eq!(
        reopened.inspect(&record.id).unwrap().state,
        ChangeState::Undone
    );
    assert_eq!(
        reopened.undo(&record.id).unwrap().state,
        ChangeState::Undone
    );
    assert_eq!(fs::read_to_string(path).unwrap(), "before");
}
