//! 独立 CLI 工具共用的版本化机器输出。

/// 编码一份 `teal.cli/v1` 成功结果。
pub(crate) fn json(
    command: &str,
    data: impl serde::Serialize,
) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&serde_json::json!({
        "schema": "teal.cli/v1",
        "command": command,
        "data": data,
    }))
}

/// 编码一份 `teal.cli/v1` 执行错误。
pub(crate) fn json_error(command: &str, kind: &str, message: &str) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "schema": "teal.cli/v1",
        "command": command,
        "error": {
            "kind": kind,
            "message": message,
        },
    }))
    .unwrap_or_else(|_| format!("{{\"schema\":\"teal.cli/v1\",\"command\":\"{command}\",\"error\":{{\"kind\":\"encoding\",\"message\":\"output encoding failed\"}}}}"))
}
