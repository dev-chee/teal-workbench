//! Teal 工作台的内部终端 UI 契约与渲染。
//!
//! 本模块提供通用面板契约、编辑器、浮层、声明式组件和帧几何。
//! 面板只发出类型化应用动作，不自行承担全局工作区策略或应用生命周期。

mod composer;
mod agent_composer;
mod containers;
mod debug_widgets;
mod decorations;
mod declarative;
mod editor;
pub use declarative::DeclarativePanel;
mod hover;
mod menu;
mod overlay;
mod panel;
mod picker;
mod prompt;
mod text;
mod tests;
mod widget;

pub use prompt::{CmdlineAction, CmdlineMode, Prompt};
pub use composer::{ComposerMode, ComposerWidget};
pub use agent_composer::AgentComposerWidget;
pub use containers::{Border, Padding, Stack, clear_area};
pub use debug_widgets::{CallStackWidget, DebugConsoleWidget, VariablesWidget};
pub use decorations::{DecorationKind, Decorations, GutterType};
pub use editor::EditorPanel;
pub use hover::HoverView;
pub use menu::{CompletionMenu, CompletionOutcome, Menu, MenuOutcome, score as fuzzy_score};
pub use overlay::{
    ConfirmOverlay, ContextMenuOverlay, Overlay, OverlayAction, OverlayStack, PermissionOverlay,
    PickerOverlay,
};
pub use panel::{
    ClipboardAction, ClipboardOutcome, ContextMenu, FrameGeometry, MenuItem, PanelKind,
    PanelStatus, PanelStore, StatusElement, WidgetAction, WindowFramePart, WindowGeometry,
    render_frame,
};
pub use picker::PickerWidget;
pub use text::{
    TextPoint, TextSelection, cell_to_char, char_to_cell, expand_line_tabs,
    render_lines_with_selection, selected_text, style_lines_with_selection, word_range,
};
pub use tests::{TestRow, TestRowStatus, TestsWidget};
pub use widget::{Split, StatusLine, Widget, WidgetChrome};
