//! 应用专用的后台任务、结果事件与观测上下文。
//!
//! 队列携带发起任务的跨度和订阅器；事件在主循环处理时恢复上下文。
//! 任务监督器管理异步任务，阻塞工作线程的结束仍由具体服务协调。

use teal_workbench_services::tools::{ApprovalPrompt, ChangeSetRequest, MutationRequest};
use teal_agent::AgentEvent;
use teal_workbench_core::Message;
use teal_lsp_client::{LspClient, LspEvent};
use std::{future::Future, sync::Arc};
use tokio::{sync::mpsc, task::JoinHandle};
use tracing::instrument::WithSubscriber as _;

pub(crate) use teal_workbench_services::tracing_context::{Context, spawn_blocking};

/// 为每条完成事件附加生产者上下文的有界发送端。
#[derive(Clone)]
pub(crate) struct RuntimeSender(mpsc::Sender<RuntimeEvent>);

impl RuntimeSender {
    /// 包装应用事件队列，不启动额外任务。
    pub fn new(sender: mpsc::Sender<RuntimeEvent>) -> Self {
        Self(sender)
    }

    /// 等待队列容量并发送事件；接收端关闭时仅返回关闭信号，不泄露事件载荷。
    pub async fn send(&self, event: RuntimeEvent) -> Result<(), mpsc::error::SendError<()>> {
        // 将生产者上下文随队列传递，供 App 处理结果。
        self.0
            .send(RuntimeEvent::Observed {
                context: Context::current(),
                event: Box::new(event),
            })
            .await
            .map_err(|_| mpsc::error::SendError(()))
    }

    /// 从 VM 调试等阻塞线程发送带上下文事件。
    pub fn blocking_send(&self, event: RuntimeEvent) -> Result<(), mpsc::error::SendError<()>> {
        self.0
            .blocking_send(RuntimeEvent::Observed {
                context: Context::current(),
                event: Box::new(event),
            })
            .map_err(|_| mpsc::error::SendError(()))
    }
}

/// 应用专用事件不会进入与协议无关的核心库。
pub(crate) enum RuntimeEvent {
    /// 扩展包预检完成，尚未批准或安装。
    ExtensionLoaded(Result<crate::app::extensions::Loaded, String>),
    /// 扩展事件的待提交业务结果。
    ExtensionEventFinished(crate::app::extensions::EventFinished),
    /// 扩展模型任务的认证预检已在创建 run 前结束。
    ExtensionAuthenticationReady {
        extension: String,
        key: String,
        result: Result<Vec<teal_agent::AuthenticatedAgentProfile>, String>,
    },
    /// 阶段任务的权威运行状态已经保存。
    ExtensionTaskFinished {
        extension: String,
        key: String,
        result: Result<(), String>,
    },
    /// 扩展任务的展示活动，不能作为业务完成依据。
    ExtensionActivity {
        extension: String,
        event: teal_agent::RuntimeAgentEvent,
    },
    /// 跨队列恢复观测上下文的信封，在应用事件边界拆开。
    Observed {
        context: Context,
        event: Box<RuntimeEvent>,
    },
    /// 已解析代码操作的后续响应，仍携带原请求供修订检查。
    LspResolved {
        request: crate::app::lsp::Request,
        result: Result<serde_json::Value, String>,
    },
    /// 行内补全结果，包含请求时的缓冲区和光标信息。
    InlineCompleted(crate::app::background::InlineResult),
    /// Agent buffer 中 Teal 代码块的独立运行结果。
    AgentCodeFinished {
        index: usize,
        result: Result<teal_workbench_services::ProcessOutput, String>,
    },
    /// 从 RuntimeStore 读取的 Graph 快照。
    AgentGraphLoaded {
        run: String,
        result: Result<Option<teal_agent::GraphSnapshot>, String>,
    },
    /// 可安装为只读面板的后台文本结果。
    TextLoaded {
        result: Result<crate::presentation::TextPanel, String>,
        axis: teal_workbench_core::SplitAxis,
    },
    /// 文件或目录读取结果，安装面板只能在应用线程完成。
    PathLoaded {
        result: Result<crate::app::files::LoadedPath, String>,
        path: std::path::PathBuf,
        generation: u64,
        address: bool,
        position: Option<serde_json::Value>,
    },
    /// 串行资源作业结束；失败也可能带回已经改变的文档副本。
    WorkspaceFinished(Result<crate::app::mutation::Completed, String>),
    /// 带面板标识和请求代次的网页结果，防止旧响应覆盖新导航。
    BrowserLoaded {
        panel: teal_workbench_core::PanelId,
        sequence: u64,
        result: Result<teal_web::WebDocument, String>,
    },
    /// 脚本执行后的注册和信任快照，界面动作仍需通过桥接处理。
    ScriptFinished(Result<crate::app::scripting::Completed, String>),
    /// 带原始请求上下文的语言服务结果。
    LspResponse(crate::app::lsp::Response),
    /// 由源码 Agent 应用发布的模型或界面事件。
    AgentAppEvent(teal_agent::RuntimeAgentEvent),
    /// 源码 Agent 的共享控制目录已就绪。
    AgentAppReady(teal_agent::RuntimeServices),
    /// 由主线程捕获当前打开缓冲区后完成的只读工作区基线请求。
    AgentAppBaseline {
        reply: tokio::sync::oneshot::Sender<Result<String, String>>,
    },
    /// 源码 Agent 已保存待答请求；后续输入将使用该身份恢复。
    AgentAppWaiting(teal_agent::InteractionRequest),
    /// 源码 Agent 应用结束或遇到阻断。
    AgentAppStopped(Result<(), String>),
    /// 内嵌 Teal 调试会话的停止或完成事件。
    Debug(crate::integration::debug::DebugEvent),
    /// 一次 `:test` 运行的结果；测试在阻塞线程上完成。
    TestsFinished {
        path: std::path::PathBuf,
        filter: Vec<String>,
        outcome: crate::app::tests_view::TestsOutcome,
    },
    /// 请求用户批准；响应通道关闭不能视为批准。
    Approval(ApprovalPrompt),
    /// 请求通过正常事务和文件队列修改工作区。
    Mutation(MutationRequest),
    /// 请求对一个多文件 ChangeSet 选择应用范围。
    ChangeSet(ChangeSetRequest),
    /// 项目 runtime lock 已经通过工作区事务写入或失败。
    RuntimeLockSaved {
        lock: teal_workbench_services::RuntimeLock,
        result: Result<(), String>,
    },
    /// 供用户查看的消息，不直接作为安全观测字段。
    Message(Message),
    /// 语言服务器主动发起的通知或请求。
    Lsp(Arc<LspClient>, LspEvent),
    /// 后台启动语言服务器的结果。
    LspStarted {
        language: String,
        result: Result<LspClient, String>,
    },
}

/// 拥有后台任务生命周期，并在应用退出时取消任务。
#[derive(Default)]
pub(crate) struct TaskSupervisor {
    tasks: Vec<JoinHandle<()>>,
}

impl TaskSupervisor {
    /// 登记并启动异步任务，清除已结束句柄但不改变活动任务顺序。
    pub(crate) fn spawn(&mut self, future: impl Future<Output = ()> + Send + 'static) {
        self.tasks.retain(|task| !task.is_finished());
        self.tasks
            .push(tokio::spawn(Context::current().instrument(future)));
    }

    /// 适配现有服务队列；转发器结束或目标关闭后不再消费来源。
    pub(crate) fn forward<T: Send + 'static>(
        &mut self,
        mut source: mpsc::Receiver<T>,
        target: RuntimeSender,
        wrap: impl Fn(T) -> RuntimeEvent + Send + 'static,
    ) {
        // 转发器没有操作父上下文。需要关联结果的生产者直接使用 RuntimeSender。
        self.tasks.push(tokio::spawn(
            async move {
                while let Some(value) = source.recv().await {
                    if target.send(wrap(value)).await.is_err() {
                        break;
                    }
                }
            }
            .with_current_subscriber(),
        ));
    }

    /// 请求取消所有异步任务，并给它们共用的一秒收尾期限。
    ///
    /// 超时后不继续等待；这不是后台阻塞线程的强制终止机制。
    pub(crate) async fn shutdown(&mut self) {
        let tasks = std::mem::take(&mut self.tasks);

        // 等待任何任务完成前，先请求取消全部任务。
        for task in &tasks {
            task.abort();
        }

        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            futures_util::future::join_all(tasks),
        )
        .await;
    }
}

impl Drop for TaskSupervisor {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}
