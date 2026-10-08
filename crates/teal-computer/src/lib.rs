//! 平台无关的桌面窗口、可访问性快照和交互能力。
#![deny(missing_docs)]

use serde::{Deserialize, Serialize};

/// 一个桌面顶层窗口的稳定快照身份。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerWindow {
    /// 所属进程 ID。
    pub process_id: u32,
    /// 本机窗口句柄的十进制字符串。
    pub window_handle: String,
    /// 当前窗口标题。
    pub title: String,
    /// 元素引用适用的代次。
    pub generation: u64,
}

/// 可访问性树中的一个有界元素记录。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerElement {
    /// UI Automation RuntimeId，以点分隔整数表示。
    pub runtime_id: String,
    /// 可访问名称。
    pub name: String,
    /// UI Automation 控件类型。
    pub control_type: String,
    /// 应用提供的 automation ID。
    pub automation_id: String,
    /// 在快照树中的深度。
    pub depth: usize,
    /// 元素是否启用。
    pub enabled: bool,
    /// 元素是否位于屏幕之外。
    pub offscreen: bool,
    /// 元素是否为密码控件；此类元素不能读取或写入值。
    pub password: bool,
}

/// 一个窗口的有界 UI Automation 快照。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerSnapshot {
    /// 更新后的窗口代次。
    pub window: ComputerWindow,
    /// 深度优先排列的元素。
    pub elements: Vec<ComputerElement>,
}

/// 桌面平台、窗口身份、元素代次或 UI Automation 操作失败。
#[derive(Debug, thiserror::Error)]
pub enum ComputerError {
    /// 必需的持久审计写入失败。
    #[error("Computer audit failed: {0}")]
    Audit(#[from] teal_security::AuditError),
    /// 桌面动作可能已发生，但审计结果无法确认。
    #[error("Computer audit result unavailable; effect may be unknown: {0}")]
    AuditUnknown(teal_security::AuditError),
    /// 当前平台没有桌面后端。
    #[error("Computer interaction is unavailable on this platform")]
    UnsupportedPlatform,
    /// UI Automation 返回失败。
    #[error("Computer UI Automation failed: {0}")]
    Automation(String),
    /// 输入调用已开始，桌面效果无法确定；调用方须先重新观察窗口。
    #[error("Computer effect may be unknown: {0}")]
    UnknownEffect(String),
    /// 窗口或元素身份无效或已经过期。
    #[error("Computer interaction target is invalid: {0}")]
    Invalid(String),
}

/// 平台桌面交互后端。
pub trait ComputerBackend: Send + Sync {
    /// 列出当前可见顶层窗口。
    fn list_windows(&self) -> Result<Vec<ComputerWindow>, ComputerError>;
    /// 获取一个有界可访问性树并推进 generation。
    fn snapshot(&self, window: &ComputerWindow) -> Result<ComputerSnapshot, ComputerError>;
    /// 使用 Invoke pattern 激活元素。
    fn invoke(&self, window: &ComputerWindow, runtime_id: &str) -> Result<(), ComputerError>;
    /// 使用 Value pattern 写入非密码元素。
    fn set_value(
        &self,
        window: &ComputerWindow,
        runtime_id: &str,
        value: &str,
    ) -> Result<(), ComputerError>;
    /// 显式使用原始鼠标点击元素中心。
    fn raw_click(&self, window: &ComputerWindow, runtime_id: &str) -> Result<(), ComputerError>;
    /// 聚焦元素后显式发送原始按键序列。
    fn raw_keys(
        &self,
        window: &ComputerWindow,
        runtime_id: &str,
        keys: &str,
    ) -> Result<(), ComputerError>;
}

/// 创建当前平台桌面后端。
#[must_use]
pub fn platform_backend() -> std::sync::Arc<dyn ComputerBackend> {
    platform_backend_with_audit(teal_security::process_context())
}

/// 使用显式宿主审计接收器创建当前平台的桌面后端。
#[must_use]
pub fn platform_backend_with_audit(
    audit: teal_security::AuditContext,
) -> std::sync::Arc<dyn ComputerBackend> {
    #[cfg(windows)]
    {
        audited_backend(
            std::sync::Arc::new(windows_backend::WindowsComputer::default()),
            audit,
        )
    }
    #[cfg(not(windows))]
    {
        let _ = audit;
        std::sync::Arc::new(UnsupportedComputer)
    }
}

/// 给自定义桌面后端的状态变更操作附加持久审计边界。
#[must_use]
pub fn audited_backend(
    inner: std::sync::Arc<dyn ComputerBackend>,
    audit: teal_security::AuditContext,
) -> std::sync::Arc<dyn ComputerBackend> {
    std::sync::Arc::new(AuditedComputer { inner, audit })
}

struct AuditedComputer {
    inner: std::sync::Arc<dyn ComputerBackend>,
    audit: teal_security::AuditContext,
}

impl AuditedComputer {
    fn run(
        &self,
        operation: impl FnOnce() -> Result<(), ComputerError>,
    ) -> Result<(), ComputerError> {
        let span = tracing::info_span!("teal.computer.action");
        let _entered = span.enter();
        let event = teal_security::Event::new(
            teal_security::Action::Computer,
            teal_security::Phase::Intent,
            teal_security::Outcome::Pending,
            teal_security::new_correlation_id(),
        )?
        .with_actor(teal_security::Actor::Host);
        let audit = self.audit.begin(event)?;
        let result = operation();
        audit
            .finish(match &result {
                Ok(()) => teal_security::Outcome::Succeeded,
                Err(ComputerError::UnknownEffect(_) | ComputerError::AuditUnknown(_)) => {
                    teal_security::Outcome::Unknown
                }
                Err(_) => teal_security::Outcome::Failed,
            })
            .map_err(ComputerError::AuditUnknown)?;
        result
    }
}

impl ComputerBackend for AuditedComputer {
    fn list_windows(&self) -> Result<Vec<ComputerWindow>, ComputerError> {
        self.inner.list_windows()
    }

    fn snapshot(&self, window: &ComputerWindow) -> Result<ComputerSnapshot, ComputerError> {
        self.inner.snapshot(window)
    }

    fn invoke(&self, window: &ComputerWindow, runtime_id: &str) -> Result<(), ComputerError> {
        self.run(|| self.inner.invoke(window, runtime_id))
    }

    fn set_value(
        &self,
        window: &ComputerWindow,
        runtime_id: &str,
        value: &str,
    ) -> Result<(), ComputerError> {
        self.run(|| self.inner.set_value(window, runtime_id, value))
    }

    fn raw_click(&self, window: &ComputerWindow, runtime_id: &str) -> Result<(), ComputerError> {
        self.run(|| self.inner.raw_click(window, runtime_id))
    }

    fn raw_keys(
        &self,
        window: &ComputerWindow,
        runtime_id: &str,
        keys: &str,
    ) -> Result<(), ComputerError> {
        self.run(|| self.inner.raw_keys(window, runtime_id, keys))
    }
}

#[cfg(not(windows))]
struct UnsupportedComputer;

#[cfg(not(windows))]
impl ComputerBackend for UnsupportedComputer {
    fn list_windows(&self) -> Result<Vec<ComputerWindow>, ComputerError> {
        Err(ComputerError::UnsupportedPlatform)
    }

    fn snapshot(&self, _: &ComputerWindow) -> Result<ComputerSnapshot, ComputerError> {
        Err(ComputerError::UnsupportedPlatform)
    }

    fn invoke(&self, _: &ComputerWindow, _: &str) -> Result<(), ComputerError> {
        Err(ComputerError::UnsupportedPlatform)
    }

    fn set_value(&self, _: &ComputerWindow, _: &str, _: &str) -> Result<(), ComputerError> {
        Err(ComputerError::UnsupportedPlatform)
    }

    fn raw_click(&self, _: &ComputerWindow, _: &str) -> Result<(), ComputerError> {
        Err(ComputerError::UnsupportedPlatform)
    }

    fn raw_keys(&self, _: &ComputerWindow, _: &str, _: &str) -> Result<(), ComputerError> {
        Err(ComputerError::UnsupportedPlatform)
    }
}

#[cfg(windows)]
mod windows_backend {
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::{Arc, Mutex, Weak},
    };

    use uiautomation::{
        UIAutomation, UIElement,
        patterns::{UIInvokePattern, UIValuePattern},
        types::Handle,
    };

    use super::{ComputerBackend, ComputerElement, ComputerError, ComputerSnapshot, ComputerWindow};

    const MAX_ELEMENTS: usize = 2_048;
    const MAX_DEPTH: usize = 32;
    const MAX_WINDOWS: usize = 512;
    type WindowIdentity = (u32, String);
    type WindowLock = Arc<Mutex<()>>;
    type WindowLocks = Mutex<BTreeMap<WindowIdentity, Weak<Mutex<()>>>>;

    #[derive(Default)]
    pub(super) struct WindowsComputer {
        generations: Mutex<BTreeMap<(u32, String), u64>>,
        // 弱引用让同窗快照和输入共用一把锁，同时不会永久保留已关闭窗口。
        operation_locks: WindowLocks,
    }

    impl ComputerBackend for WindowsComputer {
        fn list_windows(&self) -> Result<Vec<ComputerWindow>, ComputerError> {
            let automation = automation()?;
            let root = automation.get_root_element().map_err(error)?;
            let walker = automation.get_control_view_walker().map_err(error)?;
            let mut windows = walker
                .get_children(&root)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|element| window(&element).ok())
                .filter(|window| !window.title.is_empty() && window.window_handle != "0")
                .collect::<Vec<_>>();
            windows.sort_by(|left, right| {
                left.process_id
                    .cmp(&right.process_id)
                    .then_with(|| left.window_handle.cmp(&right.window_handle))
            });
            windows.truncate(MAX_WINDOWS);
            let mut generations = self.generations.lock().map_err(|_| {
                ComputerError::Invalid("computer generation state is unavailable".into())
            })?;
            let current = windows
                .iter()
                .map(|window| (window.process_id, window.window_handle.clone()))
                .collect::<BTreeSet<_>>();
            generations.retain(|identity, _| current.contains(identity));
            for window in &mut windows {
                let generation = generations
                    .entry((window.process_id, window.window_handle.clone()))
                    .or_insert(1);
                window.generation = *generation;
            }
            Ok(windows)
        }

        fn snapshot(&self, window: &ComputerWindow) -> Result<ComputerSnapshot, ComputerError> {
            let lock = self.window_lock(window)?;
            let _operation = lock.lock().map_err(|_| {
                ComputerError::Invalid("computer window operation lock is unavailable".into())
            })?;
            self.check_generation(window)?;
            let automation = automation()?;
            let root = window_element(&automation, window)?;
            let walker = automation.get_control_view_walker().map_err(error)?;
            let mut pending = vec![(root, 0_usize)];
            let mut elements = Vec::new();
            while let Some((element, depth)) = pending.pop() {
                if elements.len() >= MAX_ELEMENTS {
                    break;
                }
                elements.push(element_record(&element, depth)?);
                if depth < MAX_DEPTH {
                    let mut children = walker.get_children(&element).unwrap_or_default();
                    children.reverse();
                    pending.extend(children.into_iter().map(|child| (child, depth + 1)));
                }
            }
            let mut next = window.clone();
            next.generation = self.advance_generation(window)?;
            Ok(ComputerSnapshot {
                window: next,
                elements,
            })
        }

        fn invoke(&self, window: &ComputerWindow, runtime_id: &str) -> Result<(), ComputerError> {
            let lock = self.window_lock(window)?;
            let _operation = lock.lock().map_err(|_| {
                ComputerError::Invalid("computer window operation lock is unavailable".into())
            })?;
            let element = self.element(window, runtime_id)?;
            reject_password(&element)?;
            let pattern = element.get_pattern::<UIInvokePattern>().map_err(error)?;
            // 输入一旦交给 UIA，失败也可能已有外部效果，因此先消费代次。
            self.advance_generation(window)?;
            pattern.invoke().map_err(unknown_effect)?;
            Ok(())
        }

        fn set_value(
            &self,
            window: &ComputerWindow,
            runtime_id: &str,
            value: &str,
        ) -> Result<(), ComputerError> {
            if value.len() > 1024 * 1024 {
                return Err(ComputerError::Invalid(
                    "computer value exceeds 1 MiB".into(),
                ));
            }
            let lock = self.window_lock(window)?;
            let _operation = lock.lock().map_err(|_| {
                ComputerError::Invalid("computer window operation lock is unavailable".into())
            })?;
            let element = self.element(window, runtime_id)?;
            reject_password(&element)?;
            let pattern = element.get_pattern::<UIValuePattern>().map_err(error)?;
            self.advance_generation(window)?;
            pattern.set_value(value).map_err(unknown_effect)?;
            Ok(())
        }

        fn raw_click(
            &self,
            window: &ComputerWindow,
            runtime_id: &str,
        ) -> Result<(), ComputerError> {
            let lock = self.window_lock(window)?;
            let _operation = lock.lock().map_err(|_| {
                ComputerError::Invalid("computer window operation lock is unavailable".into())
            })?;
            let element = self.element(window, runtime_id)?;
            reject_password(&element)?;
            self.advance_generation(window)?;
            element.click().map_err(unknown_effect)?;
            Ok(())
        }

        fn raw_keys(
            &self,
            window: &ComputerWindow,
            runtime_id: &str,
            keys: &str,
        ) -> Result<(), ComputerError> {
            if keys.len() > 4096 {
                return Err(ComputerError::Invalid(
                    "raw key sequence exceeds 4096 bytes".into(),
                ));
            }
            let lock = self.window_lock(window)?;
            let _operation = lock.lock().map_err(|_| {
                ComputerError::Invalid("computer window operation lock is unavailable".into())
            })?;
            let element = self.element(window, runtime_id)?;
            reject_password(&element)?;
            self.advance_generation(window)?;
            element.set_focus().map_err(unknown_effect)?;
            element.send_keys(keys, 10).map_err(unknown_effect)?;
            Ok(())
        }
    }

    impl WindowsComputer {
        fn window_lock(&self, window: &ComputerWindow) -> Result<WindowLock, ComputerError> {
            let mut locks = self.operation_locks.lock().map_err(|_| {
                ComputerError::Invalid("computer window lock registry is unavailable".into())
            })?;
            let identity = (window.process_id, window.window_handle.clone());
            if let Some(lock) = locks.get(&identity).and_then(Weak::upgrade) {
                return Ok(lock);
            }
            locks.retain(|_, lock| lock.strong_count() > 0);
            let lock = Arc::new(Mutex::new(()));
            locks.insert(identity, Arc::downgrade(&lock));
            Ok(lock)
        }

        fn element(
            &self,
            window: &ComputerWindow,
            runtime_id: &str,
        ) -> Result<UIElement, ComputerError> {
            self.check_generation(window)?;
            let automation = automation()?;
            let root = window_element(&automation, window)?;
            let walker = automation.get_control_view_walker().map_err(error)?;
            let mut pending = vec![(root, 0_usize)];
            let wanted = parse_runtime_id(runtime_id)?;
            let mut visited = 0_usize;
            while let Some((element, depth)) = pending.pop() {
                visited += 1;
                if element.get_runtime_id().map_err(error)? == wanted {
                    return Ok(element);
                }
                if visited >= MAX_ELEMENTS || depth >= MAX_DEPTH {
                    continue;
                }
                pending.extend(
                    walker
                        .get_children(&element)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|child| (child, depth + 1)),
                );
            }
            Err(ComputerError::Invalid(
                "computer element is not in the current window snapshot".into(),
            ))
        }

        fn check_generation(&self, window: &ComputerWindow) -> Result<(), ComputerError> {
            let generations = self.generations.lock().map_err(|_| {
                ComputerError::Invalid("computer generation state is unavailable".into())
            })?;
            if generations
                .get(&(window.process_id, window.window_handle.clone()))
                .copied()
                != Some(window.generation)
            {
                return Err(ComputerError::Invalid(
                    "computer window generation is stale".into(),
                ));
            }
            Ok(())
        }

        fn advance_generation(&self, window: &ComputerWindow) -> Result<u64, ComputerError> {
            let mut generations = self.generations.lock().map_err(|_| {
                ComputerError::Invalid("computer generation state is unavailable".into())
            })?;
            let generation = generations
                .get_mut(&(window.process_id, window.window_handle.clone()))
                .ok_or_else(|| ComputerError::Invalid("computer window is unknown".into()))?;
            if *generation != window.generation {
                return Err(ComputerError::Invalid(
                    "computer window generation changed during operation".into(),
                ));
            }
            *generation = generation.saturating_add(1);
            Ok(*generation)
        }
    }

    fn automation() -> Result<UIAutomation, ComputerError> {
        UIAutomation::new().map_err(error)
    }

    fn window(element: &UIElement) -> Result<ComputerWindow, ComputerError> {
        let handle: isize = element.get_native_window_handle().map_err(error)?.into();
        Ok(ComputerWindow {
            process_id: element.get_process_id().map_err(error)?,
            window_handle: handle.to_string(),
            title: element.get_name().unwrap_or_default(),
            generation: 0,
        })
    }

    fn window_element(
        automation: &UIAutomation,
        window: &ComputerWindow,
    ) -> Result<UIElement, ComputerError> {
        let handle = window
            .window_handle
            .parse::<isize>()
            .map_err(|_| ComputerError::Invalid("computer window handle is invalid".into()))?;
        let element = automation
            .element_from_handle(Handle::from(handle))
            .map_err(error)?;
        if element.get_process_id().map_err(error)? != window.process_id {
            return Err(ComputerError::Invalid(
                "computer window process identity changed".into(),
            ));
        }
        Ok(element)
    }

    fn element_record(element: &UIElement, depth: usize) -> Result<ComputerElement, ComputerError> {
        Ok(ComputerElement {
            runtime_id: runtime_id(&element.get_runtime_id().map_err(error)?),
            name: element.get_name().unwrap_or_default(),
            control_type: element
                .get_control_type()
                .map(|value| format!("{value:?}"))
                .unwrap_or_default(),
            automation_id: element.get_automation_id().unwrap_or_default(),
            depth,
            enabled: element.is_enabled().unwrap_or(false),
            offscreen: element.is_offscreen().unwrap_or(true),
            password: element.is_password().unwrap_or(false),
        })
    }

    fn reject_password(element: &UIElement) -> Result<(), ComputerError> {
        if element.is_password().map_err(error)? {
            Err(ComputerError::Invalid(
                "password elements cannot be read or modified".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn runtime_id(values: &[i32]) -> String {
        values
            .iter()
            .map(i32::to_string)
            .collect::<Vec<_>>()
            .join(".")
    }

    fn parse_runtime_id(value: &str) -> Result<Vec<i32>, ComputerError> {
        if value.is_empty() {
            return Err(ComputerError::Invalid(
                "computer runtime ID is empty".into(),
            ));
        }
        value
            .split('.')
            .map(|part| {
                part.parse::<i32>()
                    .map_err(|_| ComputerError::Invalid("computer runtime ID is invalid".into()))
            })
            .collect()
    }

    fn error(error: impl std::fmt::Display) -> ComputerError {
        ComputerError::Automation(error.to_string())
    }

    fn unknown_effect(error: impl std::fmt::Display) -> ComputerError {
        ComputerError::UnknownEffect(error.to_string())
    }

    #[cfg(test)]
    mod tests;
}
