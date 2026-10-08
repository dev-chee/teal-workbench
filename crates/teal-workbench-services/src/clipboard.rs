//! 可替换的系统文本剪贴板接口。
//!
//! 后端初始化和读写均可能失败，调用方须先完成复制再删除选择内容，避免剪切时丢失文本。

use thiserror::Error;

/// 文本剪贴板初始化或操作失败。
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ClipboardError {
    /// 无法初始化或访问平台剪贴板。
    #[error("System clipboard is not available: {0}")]
    Unavailable(String),
}

/// 交互式应用使用的可替换 UTF-8 文本剪贴板边界。
pub trait ClipboardBackend {
    /// 读取当前纯文本剪贴板值。
    fn read_text(&mut self) -> Result<String, ClipboardError>;

    /// 用一个纯文本值替换当前剪贴板。
    fn write_text(&mut self, text: &str) -> Result<(), ClipboardError>;
}

/// 延迟初始化的操作系统文本剪贴板。首次使用后，同一个 `arboard` 句柄会保留在
/// 应用线程上，以维持 Linux 选择区所有权，并避免并发访问进程全局的 Windows 剪贴板。
#[derive(Default)]
pub struct SystemClipboard {
    clipboard: Option<arboard::Clipboard>,
}

impl SystemClipboard {
    fn clipboard(&mut self) -> Result<&mut arboard::Clipboard, ClipboardError> {
        if self.clipboard.is_none() {
            self.clipboard = Some(
                arboard::Clipboard::new()
                    .map_err(|error| ClipboardError::Unavailable(error.to_string()))?,
            );
        }
        self.clipboard
            .as_mut()
            .ok_or_else(|| ClipboardError::Unavailable("Initialization failed".to_owned()))
    }
}

impl ClipboardBackend for SystemClipboard {
    fn read_text(&mut self) -> Result<String, ClipboardError> {
        self.clipboard()?
            .get_text()
            .map_err(|error| ClipboardError::Unavailable(error.to_string()))
    }

    fn write_text(&mut self, text: &str) -> Result<(), ClipboardError> {
        self.clipboard()?
            .set_text(text)
            .map_err(|error| ClipboardError::Unavailable(error.to_string()))
    }
}
