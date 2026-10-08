@edition("2026");

host "std.bench.workspace" as workspace_host {
    task read_file(path: string) -> string;
    task read_image(path: string) -> {string: string};
    task list(path: string, recursive: bool, limit: int) -> {string: string}[];
    task stat(path: string) -> {string: string};
    task read_range(path: string, start_line: int, line_count: int) -> string;
    task search(query: string) -> string;
    task write_file(path: string, content: string) -> {string: string | none};
    task replace_file(path: string, old: string, new: string) -> {string: string | none};
    task apply_patch(path: string, expected_sha256: string, patch: string) -> {string: string | none};
    task apply_changeset(changes: string) -> string;
    task create_dir(path: string) -> string;
    task move_path(source: string, destination: string) -> string;
    task delete_path(path: string) -> string;
    task run(command: string) -> string;
    task diagnostics_json(path: string) -> string;
}

host "std.bench.toolchain" as toolchain_host {
    task check(path: string, runtime_profile: string) -> {string: int | string};
    task fmt_check(path: string) -> {string: int | string};
    task test(path: string, runtime_profile: string, filter: string) -> {string: int | string};
    task run(path: string, runtime_profile: string, capabilities: string) -> {string: int | string};
}

host "std.bench.web" as web_host {
    task fetch(url: string) -> string;
}

pub task read_file(path: string) -> string { return workspace_host::read_file(path).await; }
pub task list(path: string, recursive: bool, limit: int) -> {string: string}[] {
    return workspace_host::list(path, recursive, limit).await;
}
pub task stat(path: string) -> {string: string} {
    return workspace_host::stat(path).await;
}
pub task read_range(path: string, start_line: int, line_count: int) -> string {
    return workspace_host::read_range(path, start_line, line_count).await;
}
pub task search(query: string) -> string { return workspace_host::search(query).await; }
pub task write_file(path: string, content: string) -> string {
    return string(workspace_host::write_file(path, content).await["value"]);
}
pub task replace_file(path: string, old: string, new: string) -> string {
    return string(workspace_host::replace_file(path, old, new).await["value"]);
}
/// 应用精确的多 hunk unified diff；expected_sha256 为空时由宿主绑定当前规范化文本。
pub task apply_patch(path: string, expected_sha256: string, patch: string) -> string {
    return string(workspace_host::apply_patch(path, expected_sha256, patch).await["value"]);
}
pub task apply_changeset(changes: string) -> string {
    return workspace_host::apply_changeset(changes).await;
}
pub task create_dir(path: string) -> string { return workspace_host::create_dir(path).await; }
pub task move_path(source: string, destination: string) -> string {
    return workspace_host::move_path(source, destination).await;
}
pub task delete_path(path: string) -> string { return workspace_host::delete_path(path).await; }
pub task run(command: string) -> string { return workspace_host::run(command).await; }
pub task diagnostics(path: string) -> string {
    return workspace_host::diagnostics_json(path).await;
}
pub task teal_check(path: string, runtime_profile: string) -> {string: int | string} {
    return toolchain_host::check(path, runtime_profile).await;
}
pub task teal_fmt_check(path: string) -> {string: int | string} {
    return toolchain_host::fmt_check(path).await;
}
/// 运行脚本登记的 `@test` 测试；`filter` 为空时运行全部，否则只运行名称包含它的测试。
pub task teal_test(
    path: string,
    runtime_profile: string,
    filter: string,
) -> {string: int | string} {
    return toolchain_host::test(path, runtime_profile, filter).await;
}
pub task teal_run(
    path: string,
    runtime_profile: string,
    capabilities: string,
) -> {string: int | string} {
    return toolchain_host::run(path, runtime_profile, capabilities).await;
}
pub task fetch(url: string) -> string { return web_host::fetch(url).await; }


/// 读取工作区内 PNG/JPEG；返回原始 Base64 与尺寸摘要，Agent 桥另行附带图片。
pub task read_image(path: string) -> {string: string} { return workspace_host::read_image(path).await; }
