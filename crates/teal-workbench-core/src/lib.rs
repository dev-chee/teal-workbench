//! Teal 工作台的核心数据模型。

mod anchored;
mod buffer;
mod change_set;
mod command;
mod document;
mod event;
mod layout;
mod message;
mod mode;
mod recovery;
mod selection;
mod transaction;
mod window;
mod workspace;

#[cfg(feature = "syntax")]
pub mod syntax;

pub use buffer::{BufferHandle, BufferId, BufferStore, DocumentChange};
pub use change_set::{ChangeSet, ChangeSetError, ChangeSetFile, ChangeSetHunk, ChangeSetSelection};
pub use command::{
    AppCommand, CommandInvocation, CommandParseError, CommandRegistry, CommandScope, CommandSpec,
};
pub use document::{Document, DocumentError, LineEnding, SaveError, TextEncoding};
pub use event::{InputEvent, Key, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
pub use layout::{
    Direction, LayoutError, LayoutId, LayoutSpec, LayoutTree, Length, NodeSpec, PanelId, PanelSize,
    Rect, SplitAxis, SplitDivider, split_rectangles,
};
pub use message::{Message, MessageAction, MessageLog, Severity};
pub use mode::{EditorMode, KeymapPreset};
pub use recovery::{JournalError, RecoveryJournal};
pub use selection::{Selection, SelectionSet};
pub use transaction::{TextEdit, Transaction, TransactionError, UndoTree};
pub use window::{
    ResizeAnchor, Window, WindowAnchor, WindowChrome, WindowCloseError, WindowId,
    WindowInteraction, WindowKind, WindowLifetime, WindowPlacement, WindowSet,
};
pub use workspace::{
    ResolvedEntry, ResolvedTarget, WorkspaceIntent, WorkspaceMutationCoordinator,
    ensure_workspace_path,
};
