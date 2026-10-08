_teal() {
    local cur command options
    cur="${COMP_WORDS[COMP_CWORD]}"
    command="${COMP_WORDS[1]}"

    case "$command" in
        compile)
            options="options profiles lints --code --format --runtime-profile --optimization --debug --no-debug --debug-info --no-debug-info --allow --warn --deny --forbid --otel-traces --otel-metrics --otel-endpoint --trace-functions"
            ;;
        run)
            options="--code --bindings --runtime-profile --optimization --debug --no-debug --debug-info --no-debug-info --allow --warn --deny --forbid --permission-preset --interaction --permission-file --otel-traces --otel-metrics --otel-endpoint --trace-functions --otel-profiles --otel-profiles-endpoint --profile --profile-interval-ms"
            ;;
        perm)
            options="show preset grant explain operations revoke"
            ;;
        auth)
            options="login status logout use codex deepseek --profile --method --scope --stdin-key --env-key --yes --verify --format"
            ;;
        bench)
            options="--safe-mode --no-config --session --no-mouse"
            ;;
        *)
            options="compile run test auth perm config knowledge bench fmt lsp dap --help --version"
            ;;
    esac

    COMPREPLY=( $(compgen -W "$options" -- "$cur") )
}

complete -F _teal teal
