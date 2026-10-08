use std::io;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
};

/// 交互会话结束时恢复终端状态；丢弃时补发鼠标捕获关闭并调用 `ratatui::restore`。
pub(crate) struct TerminalGuard {
    mouse_enabled: bool,
}

impl TerminalGuard {
    pub(crate) fn enable(mouse_enabled: bool) -> io::Result<Self> {
        if mouse_enabled {
            let mut stdout = io::stdout();
            if let Err(error) = set_mouse_capture(&mut stdout, true) {
                ratatui::restore();
                return Err(error);
            }
        }
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic| {
            if mouse_enabled {
                let _ = set_mouse_capture(&mut io::stdout(), false);
            }
            previous(panic);
        }));
        Ok(Self { mouse_enabled })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.mouse_enabled {
            let _ = set_mouse_capture(&mut io::stdout(), false);
        }
        ratatui::restore();
    }
}

fn set_mouse_capture(writer: &mut impl io::Write, enabled: bool) -> io::Result<()> {
    if enabled {
        execute!(writer, EnableMouseCapture)?;
    } else {
        execute!(writer, DisableMouseCapture)?;
    }
    writer.flush()
}

#[cfg(test)]
mod tests {
    use crossterm::Command as _;

    use super::*;

    #[test]
    fn mouse_capture_commands_have_distinct_enable_and_disable_sequences() {
        let mut enabled = String::new();
        EnableMouseCapture
            .write_ansi(&mut enabled)
            .expect("enabled sequence");

        let mut disabled = String::new();
        DisableMouseCapture
            .write_ansi(&mut disabled)
            .expect("disabled sequence");

        assert!(!enabled.is_empty());
        assert!(!disabled.is_empty());
        assert_ne!(enabled, disabled);
    }
}
