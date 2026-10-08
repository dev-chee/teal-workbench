//! 版本化工作区会话及原子文件持久化。
//!
//! 加载负责格式、大小、布局标识和嵌套状态验证；根应用仍须核对活动工作区并决定是否恢复。
//! 会话不替代恢复日志，保存函数也不会自动清理用户内容中的秘密。

use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use directories::ProjectDirs;
use teal_workbench_core::{
    KeymapPreset, LayoutTree, PanelId, PanelSize, SelectionSet, SplitAxis, Window, WindowKind,
    WindowSet,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use thiserror::Error;

use crate::FileSort;

const SESSION_VERSION: u16 = 8;

/// Tests 面板允许持久化的最大测试行数。
const MAX_SESSION_TEST_ROWS: usize = 256;

/// Tests 面板允许持久化的单行输出上限。
const MAX_SESSION_TEST_ROW_OUTPUT_BYTES: usize = 64 * 1024;

/// Tests 面板允许持久化的输出总量上限。
const MAX_SESSION_TEST_OUTPUT_BYTES: usize = 1024 * 1024;

/// 一个文件树浏览器面板的持久化状态。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplorerSession {
    /// 树根目录。
    pub root: PathBuf,

    /// 应展开的目录路径。
    pub expanded: Vec<PathBuf>,

    /// 光标路径；缺失时恢复到最近的可见祖先。
    pub cursor: Option<PathBuf>,

    /// 扁平可见行中的垂直偏移。
    pub scroll: usize,

    /// 是否显示隐藏文件系统条目。
    pub show_hidden: bool,

    /// 同级条目的排序方式。
    pub sort: FileSort,
}

/// 带类型化负载的序列化面板。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PanelSession {
    /// 布局引用的面板标识。
    pub id: PanelId,

    /// 类型化持久面板状态。
    pub state: PanelSessionState,
}

/// 带标签的面板负载；运行期缓冲区 ID 会重新建立。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PanelSessionState {
    /// 编辑器资源与视图状态。
    Editor {
        /// 包括未命名缓冲区的稳定不透明恢复标识；不持久化运行期 ID。
        recovery_key: Option<String>,

        /// 后备文件或未命名临时缓冲区。
        path: Option<PathBuf>,

        /// 文本选择区。
        selections: SelectionSet,

        /// 垂直视图偏移。
        scroll: usize,

        /// 水平视图偏移。
        column: usize,
    },

    /// 文件树视图状态。
    Explorer {
        /// 树状态。
        tree: ExplorerSession,
    },

    /// 离线浏览器状态。
    Browser {
        /// 导航历史。
        browser: teal_web::BrowserSession,

        /// 垂直视图偏移。
        scroll: usize,

        /// 水平视图偏移。
        column: usize,
    },

    /// 只读文本或输出视图。
    Text {
        /// 原面板类别。
        category: String,

        /// 面板标题。
        title: String,

        /// 已渲染纯文本。
        content: String,

        /// 面板拥有的可选结构化内容；普通文本面板不携带该字段。
        #[serde(skip_serializing_if = "Option::is_none")]
        blocks: Option<serde_json::Value>,

        /// 逻辑资源。
        resource: String,

        /// 垂直偏移。
        scroll: usize,

        /// 水平偏移。
        column: usize,
    },

    /// 仅在注册项可用后恢复的富交互 Teal 面板。
    TealCustom {
        /// 激活注册名称。
        name: String,

        /// 最近校验的组件规格。
        specification: String,

        /// 有界局部控件状态。
        widget_state: serde_json::Value,
    },

    /// `@test` 运行结果列表；恢复后仍可跳转、重跑与滚动。
    Tests {
        /// 按源码声明顺序排列的测试结果行。
        rows: Vec<TestSessionRow>,

        /// 选中行下标；列表为空时为 0。
        selected: usize,

        /// 垂直滚动偏移，单位为显示行。
        scroll: usize,

        /// 本次运行使用的名称过滤器说明。
        filter: String,

        /// 列表末尾显示的运行摘要。
        summary: String,

        /// 产生这次的源码路径，供恢复后重新编译运行。
        #[serde(skip_serializing_if = "Option::is_none")]
        source: Option<PathBuf>,
    },
}

/// 一个测试结果行的持久化投影。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestSessionRow {
    /// 测试名称。
    pub name: String,

    /// 声明所在源码文件的显示名称；列表按它分组。
    pub source: String,

    /// 声明所在文件的规范路径；缺失时无法跳转。
    pub path: Option<PathBuf>,

    /// 声明行号，从 1 开始。
    pub line: u32,

    /// 执行状态。
    pub status: TestSessionStatus,

    /// 失败说明；通过时为空。
    pub message: String,

    /// 测试实例的控制台输出。
    pub output: String,
}

/// 测试行的稳定执行状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum TestSessionStatus {
    /// 测试返回 `0`。
    Passed,

    /// 测试抛出未捕获错误或执行失败。
    Failed,

    /// 测试超出指令预算或墙钟上限。
    TimedOut,

    /// 测试返回非零退出码。
    ExitCode {
        /// 测试函数返回的退出码。
        code: i32,
    },
}

/// 带版本的工作区会话状态。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionState {
    /// 序列化版本。
    pub version: u16,

    /// 规范工作区根目录。
    pub workspace: PathBuf,

    /// 按显示顺序排列的窗口；每个窗口拥有自己的拆分树和聚焦面板标识。
    pub windows: WindowSet,

    /// 按应用拥有顺序排列的面板负载。
    pub panels: Vec<PanelSession>,

    /// 当前随附键位预设。
    pub keymap: KeymapPreset,

    /// 旧版 Bench Agent 对话，仅用于兼容检测；源码 Agent 不从这里恢复。
    ///
    /// 新会话保持为空且省略该字段。读取到非空值时，宿主必须拒绝恢复并保留原文件。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<String, teal_agent::AgentSession>,

    /// 保存时选中的命名 Agent。
    pub active_agent: Option<String>,
}

impl SessionState {
    /// 创建当前版本的会话。
    #[must_use]
    pub fn new(workspace: PathBuf, windows: WindowSet) -> Self {
        Self {
            version: SESSION_VERSION,
            workspace,
            windows,
            panels: Vec::new(),
            keymap: KeymapPreset::default(),
            agents: BTreeMap::new(),
            active_agent: None,
        }
    }
}

/// Teal 工作台配置、状态、缓存与日志的标准按用户路径。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionPaths {
    /// 漫游配置目录。
    pub config: PathBuf,

    /// 本地状态目录。
    pub state: PathBuf,

    /// 缓存目录。
    pub cache: PathBuf,
}

impl SessionPaths {
    /// 解析平台标准路径。
    #[must_use]
    pub fn discover() -> Option<Self> {
        let project = ProjectDirs::from("dev", "Teal", "Bench")?;
        Some(Self {
            config: project.config_dir().to_path_buf(),
            state: project.data_local_dir().join("state"),
            cache: project.cache_dir().to_path_buf(),
        })
    }

    /// 返回一个工作区的确定性会话路径。
    #[must_use]
    pub fn session_for(&self, workspace: &Path) -> PathBuf {
        use std::fmt::Write as _;

        let hash = Sha256::digest(workspace.to_string_lossy().as_bytes());
        let mut name = String::with_capacity(24);
        for byte in &hash[..12] {
            write!(name, "{byte:02x}").expect("Writing to string does not fail");
        }
        self.state.join("sessions").join(format!("{name}.json"))
    }
}

/// 会话访问或兼容性失败。
#[derive(Debug, Error)]
pub enum SessionError {
    /// 文件系统访问失败。
    #[error("Session read and write failed: {0}")]
    Io(#[from] std::io::Error),

    /// JSON 解码失败。
    #[error("Session data format error: {0}")]
    Json(#[from] serde_json::Error),

    /// 遇到未来或不受支持的模式。
    #[error("Unsupported session version {0}")]
    Version(u16),

    /// 新会话不能写入旧版 Bench Agent 对话。
    #[error("Legacy Bench Agent sessions cannot be saved")]
    LegacyAgents,
}

/// 原子写入给定会话；新会话不能写入旧版 Bench Agent 对话。
pub fn save_session(path: &Path, state: &SessionState) -> Result<(), SessionError> {
    if state.version != SESSION_VERSION {
        return Err(SessionError::Version(state.version));
    }
    if !state.agents.is_empty() {
        return Err(SessionError::LegacyAgents);
    }

    let bytes = serde_json::to_vec_pretty(state)?;
    if bytes.len() >= 64 * 1024 * 1024 {
        return Err(
            std::io::Error::new(std::io::ErrorKind::InvalidData, "Session exceeds 64 MiB").into(),
        );
    }

    // 先序列化并检查边界，再在现有会话文件旁创建替换文件。
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut temporary = NamedTempFile::new_in(parent)?;
    temporary.write_all(&bytes)?;
    temporary.write_all(b"\n")?;
    temporary.as_file_mut().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| SessionError::Io(error.error))?;

    Ok(())
}

/// 加载当前 Teal 工作台会话。
///
/// 读取受 64 MiB 上限约束，拒绝不支持版本、重复或不匹配的面板身份及越界嵌套状态。
/// 浏览器仅恢复离线快照，智能体未完成调用标记为结果未知；此处不安装面板，也不核对当前活动工作区。
pub fn load_session(path: &Path) -> Result<SessionState, SessionError> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(64 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err(
            std::io::Error::new(std::io::ErrorKind::InvalidData, "Session exceeds 64 MiB").into(),
        );
    }

    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| u16::try_from(v).ok())
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid session version")
        })?;
    if version != SESSION_VERSION {
        return Err(SessionError::Version(version));
    }
    let mut state: SessionState = serde_json::from_value(value)?;

    // 加载当前结构后，仍须验证恢复的标识与布局一致。
    let invalid = |reason: String| {
        SessionError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, reason))
    };
    let ids = state
        .windows
        .validate()
        .map_err(|e| invalid(e.to_string()))?
        .into_iter()
        .flat_map(|(_, panels)| panels)
        .collect::<Vec<_>>();
    let panel_ids = state
        .panels
        .iter()
        .map(|panel| panel.id)
        .collect::<std::collections::HashSet<_>>();
    if panel_ids.len() != state.panels.len()
        || ids.len() != panel_ids.len()
        || ids.iter().any(|id| !panel_ids.contains(id))
    {
        return Err(invalid(
            "Layout is inconsistent with panel identification".into(),
        ));
    }

    for panel in &state.panels {
        match &panel.state {
            PanelSessionState::Browser { browser, .. } => browser.validate().map_err(invalid)?,
            PanelSessionState::TealCustom {
                specification,
                widget_state,
                ..
            } if specification.len() > 2 * 1024 * 1024
                || widget_state.to_string().len() > 256 * 1024 =>
            {
                return Err(invalid("Custom panel exceeds limit".into()));
            }
            PanelSessionState::Tests {
                rows,
                selected,
                summary,
                ..
            } => {
                if rows.len() > MAX_SESSION_TEST_ROWS
                    || *selected >= rows.len().max(1)
                    || summary.len() > 4 * 1024
                    || rows
                        .iter()
                        .any(|row| row.output.len() > MAX_SESSION_TEST_ROW_OUTPUT_BYTES)
                {
                    return Err(invalid("Test panel exceeds limit".into()));
                }
                let output_bytes = rows.iter().map(|row| row.output.len()).sum::<usize>();
                if output_bytes > MAX_SESSION_TEST_OUTPUT_BYTES {
                    return Err(invalid("Test panel output exceeds limit".into()));
                }
            }
            _ => {}
        }
    }
    for agent in state.agents.values_mut() {
        agent.prepare_restore_unbound().map_err(invalid)?;
    }
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use slotmap::KeyData;
    use tempfile::tempdir;

    fn test_model_session(model: &str) -> teal_model::ModelSessionContext {
        teal_model::ModelSessionContext {
            target: teal_model::ModelRef {
                provider: teal_model::ProviderId::new("test-provider").unwrap(),
                model: teal_model::ModelId::new(model).unwrap(),
            },
            auth_profile: teal_model::AuthProfileName::new("default").unwrap(),
            service: teal_model::ModelServiceContract {
                endpoint: "https://models.example.test/".into(),
                protocol: teal_model::ProtocolId::new("responses").unwrap(),
                transport: "http".into(),
            },
            account_identity: "test-account".into(),
            account: teal_model::AuthAccount {
                account_id: Some("test-account".into()),
                email: None,
                plan_type: Some("test".into()),
            },
        }
    }

    fn editor_session(id: PanelId) -> PanelSession {
        PanelSession {
            id,
            state: PanelSessionState::Editor {
                recovery_key: None,
                path: None,
                selections: SelectionSet::default(),
                scroll: 0,
                column: 0,
            },
        }
    }

    fn editor_windows(layout: LayoutTree) -> WindowSet {
        WindowSet::new(Window::new(
            WindowKind::Editor,
            WindowKind::Editor.default_title(),
            layout,
        ))
    }

    #[test]
    fn platform_paths_are_scoped_to_bench_not_the_legacy_ide_directory() {
        let paths = SessionPaths::discover().expect("platform paths should be available");
        let has_bench_root = |path: &Path| {
            path.components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .windows(2)
                .any(|components| components[0] == "Teal" && components[1] == "Bench")
        };
        assert!(has_bench_root(&paths.config));
        assert!(has_bench_root(&paths.state));
        assert!(has_bench_root(&paths.cache));
        assert!(paths.config.ends_with("config"));
        assert!(paths.state.ends_with("state"));
        assert!(paths.cache.ends_with("cache"));
        for path in [&paths.config, &paths.state, &paths.cache] {
            assert!(
                !path
                    .components()
                    .any(|component| component.as_os_str() == "IDE"),
                "legacy IDE path was selected: {}",
                path.display()
            );
        }
    }

    #[test]
    fn current_round_trip_preserves_layout_and_resources() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.json");
        let id = PanelId::from(KeyData::from_ffi(1));
        let mut state =
            SessionState::new(dir.path().to_owned(), editor_windows(LayoutTree::new(id)));
        state.panels.push(PanelSession {
            id,
            state: PanelSessionState::Editor {
                recovery_key: None,
                path: Some("\u{4e16}\u{754c}.rs".into()),
                selections: SelectionSet::default(),
                scroll: 3,
                column: 2,
            },
        });
        save_session(&path, &state).unwrap();
        let restored = load_session(&path).unwrap();
        assert_eq!(restored.version, SESSION_VERSION);
        assert_eq!(restored.panels, state.panels);
    }

    #[test]
    fn text_blocks_round_trip_and_are_optional_for_sessions_without_the_field() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.json");
        let id = PanelId::from(KeyData::from_ffi(1));
        let blocks = serde_json::json!([
            {"markdown": "saved reply"},
            {"approval": {"id": "call-1", "decision": "interrupted"}}
        ]);
        let mut state =
            SessionState::new(dir.path().to_owned(), editor_windows(LayoutTree::new(id)));
        state.panels.push(PanelSession {
            id,
            state: PanelSessionState::Text {
                category: "agent".into(),
                title: "Agent".into(),
                content: String::new(),
                blocks: Some(blocks.clone()),
                resource: String::new(),
                scroll: 0,
                column: 0,
            },
        });

        save_session(&path, &state).unwrap();
        let restored = load_session(&path).unwrap();
        assert_eq!(restored.panels, state.panels);

        // 普通文本面板允许缺少可选的结构化块字段。
        let mut without_blocks = serde_json::to_value(&state).unwrap();
        without_blocks["panels"][0]["state"]
            .as_object_mut()
            .unwrap()
            .remove("blocks");
        fs::write(&path, without_blocks.to_string()).unwrap();
        let restored = load_session(&path).unwrap();
        assert!(matches!(
            &restored.panels[0].state,
            PanelSessionState::Text { blocks: None, .. }
        ));
    }

    #[test]
    fn rejects_unsupported_versions_and_corrupt_data() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.json");
        // 只有当前版本可以加载；历史版本与未来版本都整体拒绝，不做迁移。
        for version in [1, 2, 3, 4, 5, 6, 7, SESSION_VERSION + 1] {
            fs::write(&path, format!("{{\"version\":{version}}}")).unwrap();
            assert!(matches!(load_session(&path),Err(SessionError::Version(v)) if v==version));
        }
        fs::write(&path, b"corrupt").unwrap();
        assert!(load_session(&path).is_err());
    }

    fn tests_panel(id: PanelId, rows: Vec<TestSessionRow>) -> PanelSession {
        PanelSession {
            id,
            state: PanelSessionState::Tests {
                rows,
                selected: 0,
                scroll: 2,
                filter: "addition".into(),
                summary: "1 passed; 0 failed; 1 selected".into(),
                source: Some("main.tl".into()),
            },
        }
    }

    fn test_row(name: &str, output: String) -> TestSessionRow {
        TestSessionRow {
            name: name.into(),
            source: "main.tl".into(),
            path: Some("main.tl".into()),
            line: 3,
            status: TestSessionStatus::Passed,
            message: String::new(),
            output,
        }
    }

    #[test]
    fn tests_panel_round_trips_and_rejects_version_five() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.json");
        let panel = PanelId::from(KeyData::from_ffi(1));
        let mut state = SessionState::new(
            dir.path().to_owned(),
            editor_windows(LayoutTree::new(panel)),
        );
        state.panels.push(tests_panel(
            panel,
            vec![test_row("passes", "checked\n".into())],
        ));

        save_session(&path, &state).unwrap();
        let restored = load_session(&path).unwrap();
        assert_eq!(restored.version, SESSION_VERSION);
        assert_eq!(restored.panels, state.panels);

        // v5 会话不再迁移：整体拒绝，磁盘内容保持不变。
        let mut legacy = serde_json::to_value(&state).unwrap();
        legacy["version"] = serde_json::json!(5);
        let legacy_text = legacy.to_string();
        fs::write(&path, &legacy_text).unwrap();
        assert!(matches!(load_session(&path), Err(SessionError::Version(5))));
        assert_eq!(fs::read_to_string(&path).unwrap(), legacy_text);
    }

    #[test]
    fn rejects_oversized_or_inconsistent_tests_panel_payloads() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.json");
        let panel = PanelId::from(KeyData::from_ffi(1));
        let windows = editor_windows(LayoutTree::new(panel));

        let too_many = (0..=MAX_SESSION_TEST_ROWS)
            .map(|index| test_row(&format!("case{index}"), String::new()))
            .collect::<Vec<_>>();
        let mut state = SessionState::new(dir.path().to_owned(), windows.clone());
        state.panels.push(tests_panel(panel, too_many));
        save_session(&path, &state).unwrap();
        assert!(load_session(&path).is_err());

        let oversized_row = vec![test_row(
            "huge",
            "x".repeat(MAX_SESSION_TEST_ROW_OUTPUT_BYTES + 1),
        )];
        let mut state = SessionState::new(dir.path().to_owned(), windows.clone());
        state.panels.push(tests_panel(panel, oversized_row));
        save_session(&path, &state).unwrap();
        assert!(load_session(&path).is_err());

        let oversized_total = (0..32)
            .map(|index| {
                test_row(
                    &format!("case{index}"),
                    "y".repeat(MAX_SESSION_TEST_ROW_OUTPUT_BYTES),
                )
            })
            .collect::<Vec<_>>();
        let mut state = SessionState::new(dir.path().to_owned(), windows.clone());
        state.panels.push(tests_panel(panel, oversized_total));
        save_session(&path, &state).unwrap();
        assert!(load_session(&path).is_err());

        let mut state = SessionState::new(dir.path().to_owned(), windows);
        let mut inconsistent = tests_panel(panel, vec![test_row("passes", String::new())]);
        let PanelSessionState::Tests { selected, .. } = &mut inconsistent.state else {
            unreachable!("the fixture builds a Tests panel")
        };
        *selected = 3;
        state.panels.push(inconsistent);
        save_session(&path, &state).unwrap();
        assert!(load_session(&path).is_err());
    }

    #[test]
    fn version_two_session_is_rejected_without_replaying_nested_agents() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.json");
        let id = PanelId::from(KeyData::from_ffi(1));
        for (name, status) in [
            ("completed", teal_agent::AgentSessionStatus::Completed),
            ("running", teal_agent::AgentSessionStatus::Running),
        ] {
            let mut state =
                SessionState::new(dir.path().to_owned(), editor_windows(LayoutTree::new(id)));
            state.panels.push(PanelSession {
                id,
                state: PanelSessionState::Editor {
                    recovery_key: None,
                    path: None,
                    selections: SelectionSet::default(),
                    scroll: 0,
                    column: 0,
                },
            });
            let config = teal_agent::AgentConfig {
                model: test_model_session("model"),
                compaction_model: None,
                instructions: "fixed".into(),
                limits: teal_agent::RuntimeLimits::default(),
            };
            let mut agent = teal_agent::AgentSession::capture(
                &config,
                vec![teal_model::ModelMessage::User {
                    text: "saved".into(),
                }],
                status,
                Vec::new(),
                Vec::new(),
            );
            agent.version = 2;
            state.agents.insert(name.into(), agent);
            let mut value = serde_json::to_value(state).unwrap();
            value["version"] = serde_json::json!(2);
            let text = value.to_string();
            fs::write(&path, &text).unwrap();
            // 旧工作台会话整体拒绝：既不再解释嵌套 Agent 会话，也不会重放在途调用。
            assert!(matches!(load_session(&path), Err(SessionError::Version(2))));
            assert_eq!(fs::read_to_string(&path).unwrap(), text);
        }
    }
}
