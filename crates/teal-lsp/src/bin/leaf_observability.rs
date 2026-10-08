//! 独立命令的本地日志与显式 OTLP 装配；协议标准输出保持纯净。

pub(crate) fn start() -> Result<teal_workbench_services::observability::Guard, String> {
    let paths = teal_config::ConfigPaths::discover(None).map_err(|error| error.to_string())?;
    let workspace = std::env::current_dir().map_err(|error| error.to_string())?;
    let args = teal_workbench_services::observability::Arguments {
        otel_traces: std::env::var("TEAL_OTEL_TRACES").is_ok_and(|value| value == "1"),
        otel_logs: std::env::var("TEAL_OTEL_LOGS").is_ok_and(|value| value == "1"),
        otel_metrics: std::env::var("TEAL_OTEL_METRICS").is_ok_and(|value| value == "1"),
        ..Default::default()
    };
    let options = teal_workbench_services::observability::Options::resolve(
        args,
        workspace,
        paths.teal_home.join("logs"),
        |name| std::env::var(name).ok(),
    )
    .map_err(|error| error.to_string())?;
    teal_workbench_services::observability::Guard::initialize(options)
        .map_err(|error| error.to_string())
}
