//! 与终端后端无关的键盘、鼠标和剪贴板动作。
//!
//! 此处只描述输入，不绑定 Crossterm 或面板；上层根据最近一次渲染几何决定输入归属。

/// 独立于终端后端的可移植按键标识。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// 可打印 Unicode 字符。
    Char(char),

    /// 回车键。
    Enter,

    /// Esc 键。
    Escape,

    /// 退格键。
    Backspace,

    /// 删除键。
    Delete,

    /// 方向键。
    Up,

    /// 方向键。
    Down,

    /// 方向键。
    Left,

    /// 方向键。
    Right,

    /// Home 键。
    Home,

    /// End 键。
    End,

    /// 向上翻页。
    PageUp,

    /// 向下翻页。
    PageDown,

    /// Tab 键。
    Tab,

    /// 反向 Tab。
    BackTab,

    /// 功能键。
    Function(u8),
}

/// 可移植按键修饰符。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct KeyModifiers {
    /// Control 键。
    pub control: bool,

    /// Alt 键。
    pub alt: bool,

    /// Shift 键。
    pub shift: bool,
}

/// 一次规范化按键。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct InputEvent {
    /// 按键标识。
    pub key: Key,

    /// 当前修饰符。
    pub modifiers: KeyModifiers,
}

/// 独立于终端后端的可移植鼠标按钮标识。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseButton {
    /// 主鼠标按钮。
    Left,

    /// 次鼠标按钮。
    Right,

    /// 鼠标中键。
    Middle,
}

/// 独立于终端后端的可移植鼠标事件类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseEventKind {
    /// 按钮被按下。
    Down(MouseButton),

    /// 按钮被释放。
    Up(MouseButton),

    /// 按住按钮时移动指针。
    Drag(MouseButton),

    /// 未按住按钮时移动指针。
    Moved,

    /// 垂直滚轮向上滚动。
    ScrollUp,

    /// 垂直滚轮向下滚动。
    ScrollDown,

    /// 水平滚轮或触控板向左移动。
    ScrollLeft,

    /// 水平滚轮或触控板向右移动。
    ScrollRight,
}

/// 使用绝对终端单元格坐标的一次规范化鼠标事件。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MouseEvent {
    /// 事件类型和可选按钮。
    pub kind: MouseEventKind,

    /// 从零开始的终端列。
    pub column: u16,

    /// 从零开始的终端行。
    pub row: u16,

    /// 随指针事件报告的当前键盘修饰符。
    pub modifiers: KeyModifiers,

    /// 按钮按下事件的连续点击数，限制为 `1..=3`。
    pub click_count: u8,
}
