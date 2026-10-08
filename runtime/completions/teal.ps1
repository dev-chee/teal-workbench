Register-ArgumentCompleter -Native -CommandName teal -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)

    $arguments = @($commandAst.CommandElements | Select-Object -Skip 1 | ForEach-Object { $_.Extent.Text })
    $command = if ($arguments.Count -gt 0) { $arguments[0] } else { "" }
    $candidates = switch ($command) {
        "compile" { @("options", "profiles", "lints", "--code", "--runtime-profile", "--optimization", "--debug", "--no-debug", "--debug-info", "--no-debug-info", "--allow", "--warn", "--deny", "--forbid", "--otel-traces", "--otel-metrics", "--otel-endpoint", "--trace-functions") }
        "run" { @("--code", "--bindings", "--runtime-profile", "--optimization", "--debug", "--no-debug", "--debug-info", "--no-debug-info", "--allow", "--warn", "--deny", "--forbid", "--permission-preset", "--interaction", "--permission-file", "--otel-traces", "--otel-metrics", "--otel-endpoint", "--trace-functions", "--otel-profiles", "--otel-profiles-endpoint", "--profile", "--profile-interval-ms") }
        "test" { @("--code", "--filter", "--format", "--profile", "--runtime-profile", "--optimization", "--permission-preset", "--interaction", "--permission-file") }
        "perm" { @("show", "preset", "grant", "explain", "operations", "revoke") }
        "auth" { @("login", "status", "logout", "use", "codex", "deepseek", "--profile", "--method", "--scope", "--stdin-key", "--env-key", "--yes", "--verify", "--format") }
        "config" { @("list", "get", "set", "unset", "paths") }
        "knowledge" { @("status", "verify", "search", "read", "lookup", "export", "add", "write", "refresh") }
        "bench" { @("--safe-mode", "--no-config", "--session", "--no-mouse") }
        default { @("compile", "run", "test", "auth", "perm", "config", "knowledge", "bench", "fmt", "lsp", "dap", "--help", "--version") }
    }

    $candidates |
        Where-Object { $_ -like "$wordToComplete*" } |
        ForEach-Object {
            [System.Management.Automation.CompletionResult]::new($_, $_, "ParameterValue", $_)
        }
}
