//! 进程资源的启动和退出顺序；终端恢复先于运行时和观测关闭。

use std::path::PathBuf;
use tracing::Instrument as _;
use crate::{
    StartupOptions,
    app::{App, AppOptions, bootstrap::ensure_workspace_trust},
    terminal::TerminalGuard,
};
use teal_workbench_services::observability;

/// 启动工作台，并通过明确的资源所有权管理进程生命周期。
///
/// 观测先于应用构建启动，以便记录启动过程。仅在 `App` 准备完成后才初始化终端。
/// 事件循环返回后，无论成功还是失败都等待应用清理；仅运行成功时请求保存会话。
/// 运行错误优先于清理错误返回。
///
/// 离开 `block_on` 前先析构终端守卫。工作区已开始的写入会继续结算；30 秒只用于发出
/// 等待提示，不强制停止阻塞任务。完成收尾后再关闭运行时和观测。
///
/// # Errors
/// 工作区信任、观测配置、异步运行时、应用或终端初始化失败，以及事件循环或清理失败时
/// 返回错误。调用方负责把错误转换为适合其进程边界的输出与退出码。
pub fn run(args: StartupOptions) -> Result<(), Box<dyn std::error::Error>> {
    let log_directory = teal_workbench_services::SessionPaths::discover().map_or_else(
        || PathBuf::from(".teal/bench/logs"),
        |paths| paths.state.join("logs"),
    );

    // 解析观测使用的工作区标识，保留交给 App 的原始路径。
    let requested = args.path.clone().unwrap_or(std::env::current_dir()?);
    let requested = requested.canonicalize().unwrap_or(requested);
    let workspace = if requested.is_dir() {
        requested
    } else {
        requested
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_path_buf()
    };
    if !args.safe_mode && !args.no_config {
        ensure_workspace_trust(&workspace)?;
    }

    let observation = observability::Guard::initialize(observability::Options::resolve(
        args.observation,
        workspace,
        log_directory,
        |name| std::env::var(name).ok(),
    )?)?;

    // 显式拥有运行时，使观测资源存活到应用和运行时清理结束之后。
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let result = runtime.block_on(async {
        let options = AppOptions {
            // 显式传入文件时主窗口编辑该文件；传入目录或省略时落到 Agent 主窗口。
            start_file: args.path.as_ref().filter(|path| path.is_file()).cloned(),
            path: args.path,
            safe_mode: args.safe_mode,
            no_config: args.no_config,
            session: args.session,
            model_protocol_capture: args.model_protocol_capture,
        };

        // 普通构造路径仍使用进程日志；可选的是传给 Teal 的观测接口。
        let mut app = async {
            let result = if args.offline {
                App::new_with_connectivity(
                    options,
                    observation.meter(),
                    observation.function_spans,
                    teal_workbench_services::agent_algorithms::ConnectivityMode::Offline,
                )
                .await
            } else if observation.meter().is_none() && !observation.function_spans {
                App::new(options).await
            } else {
                App::new_with_connectivity(
                    options,
                    observation.meter(),
                    observation.function_spans,
                    teal_workbench_services::agent_algorithms::ConnectivityMode::Online,
                )
                .await
            };

            observability::outcome(&result);
            result
        }
        // 每次轮询时进入启动跨度，由 outcome 填写预留的结果字段。
        .instrument(tracing::info_span!(
            "teal.bench.startup",
            operation.result = tracing::field::Empty,
            otel.status_code = tracing::field::Empty
        ))
        .await?;
        app.set_observation_health(observation.health());

        let mut terminal = match ratatui::try_init() {
            Ok(terminal) => terminal,
            Err(error) => {
                ratatui::restore();
                // App 可能已拥有后台任务；执行清理，但保留终端启动错误。
                let _ = app.shutdown(false).await;
                return Err(error.into());
            }
        };

        // 具名绑定让守卫存活到异步作用域结束，覆盖应用清理阶段。
        let _terminal_guard = match TerminalGuard::enable(args.capture_mouse) {
            Ok(guard) => guard,
            Err(error) => {
                let _ = app.shutdown(false).await;
                return Err(error.into());
            }
        };

        // 此处不能用 `?` 提前返回：运行失败后仍须收尾应用拥有的任务。
        let result = app.run(&mut terminal).await;
        let shutdown = app.shutdown(result.is_ok()).await;

        // `save` 控制会话持久化，不是保存所有文档；最终优先保留运行错误。
        result.and(shutdown)
    });

    // 在关闭运行时和输出最终观测诊断之前，终端已经恢复。
    runtime.shutdown_timeout(std::time::Duration::from_secs(1));
    observation.shutdown();

    result
}
