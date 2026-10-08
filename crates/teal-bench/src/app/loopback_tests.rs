use teal_workbench_services::agent_algorithms::allows_offline_model_tool;

#[test]
fn offline_loopback_tool_directory_allows_only_local_workspace_and_verification_tools() {
    for tool in [
        "workspace.read_file",
        "workspace.list",
        "workspace.stat",
        "workspace.read_range",
        "workspace.search",
        "workspace.write_file",
        "workspace.replace_file",
        "workspace.apply_patch",
        "workspace.apply_changeset",
        "workspace.create_dir",
        "workspace.move",
        "workspace.delete",
        "workspace.diagnostics",
        "teal.fmt_check",
        "teal.check",
    ] {
        assert!(allows_offline_model_tool(tool), "must allow {tool}");
    }
    for tool in [
        "workspace.run",
        "process.exec",
        "web.fetch",
        "browser.open",
        "teal.run",
        "git.commit",
    ] {
        assert!(!allows_offline_model_tool(tool), "must reject {tool}");
    }
}
