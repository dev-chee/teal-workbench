@edition("2026");

host "std.bench.workspace" as workspace_host {
    task read_file(path: string) -> string | Error;

    task read_image(path: string) -> { string: string } | Error;
    task list(path: string, recursive: bool, limit: int) -> { string: string }[] | Error;
    task stat(path: string) -> { string: string } | Error;
    task read_range(path: string, start_line: int, line_count: int) -> string | Error;

    task search(query: string) -> string | Error;

    task write_file(path: string, content: string) -> { string: string | none } | Error;
    task replace_file(path: string, old: string, new: string) -> { string: string | none } | Error;
    task apply_patch(path: string, expected_sha256: string, patch: string) -> { string: string | none } | Error;
    task apply_changeset(changes: string) -> string | Error;

    task create_dir(path: string) -> string | Error;

    task move_path(source: string, destination: string) -> string | Error;

    task delete_path(path: string) -> string | Error;

    task run(command: string) -> string | Error;

    task diagnostics_json(path: string) -> string | Error;
} host "std.bench.toolchain" as toolchain_host {
    task check(path: string, tier: string) -> { string: int | string } | Error;
    task fmt_check(path: string) -> { string: int | string } | Error;
    task test(path: string, tier: string, filter: string) -> { string: int | string } | Error;
    task run(path: string, tier: string, permission_file: string) -> { string: int | string } | Error;
} host "std.bench.web" as web_host {
    task fetch(url: string) -> string | Error;
}

pub task read_file(path: string) -> string | Error {
    return workspace_host::read_file(path).await ?;
}

pub task list(path: string, recursive: bool, limit: int) -> { string: string }[] | Error {
    return workspace_host::list(path, recursive, limit).await ?;
}
pub task stat(path: string) -> { string: string } | Error {
    return workspace_host::stat(path).await ?;
}
pub task read_range(path: string, start_line: int, line_count: int) -> string | Error {
    return workspace_host::read_range(path, start_line, line_count).await ?;
}

pub task search(query: string) -> string | Error {
    return workspace_host::search(query).await ?;
}

pub task write_file(path: string, content: string) -> string | Error {
    return string(workspace_host::write_file(path, content).await ? ["value"]);
}

pub task replace_file(path: string, old: string, new: string) -> string | Error {
    return string(workspace_host::replace_file(path, old, new).await ? ["value"]);
}

/// 应用精确的多 hunk unified diff；expected_sha256 为空时由宿主绑定当前规范化文本。
pub task apply_patch(path: string, expected_sha256: string, patch: string) -> string | Error {
    return string(workspace_host::apply_patch(path, expected_sha256, patch).await ? ["value"]);
}

pub task apply_changeset(changes: string) -> string | Error {
    return workspace_host::apply_changeset(changes).await ?;
}

pub task create_dir(path: string) -> string | Error {
    return workspace_host::create_dir(path).await ?;
}

pub task move_path(source: string, destination: string) -> string | Error {
    return workspace_host::move_path(source, destination).await ?;
}

pub task delete_path(path: string) -> string | Error {
    return workspace_host::delete_path(path).await ?;
}

pub task run(command: string) -> string | Error {
    return workspace_host::run(command).await ?;
}

pub task diagnostics(path: string) -> string | Error {
    return workspace_host::diagnostics_json(path).await ?;
}

pub task teal_check(path: string, tier: string) -> { string: int | string } | Error {
    return toolchain_host::check(path, tier).await ?;
}
pub task teal_fmt_check(path: string) -> { string: int | string } | Error {
    return toolchain_host::fmt_check(path).await ?;
}
/// 运行脚本登记的 `@test` 测试；`filter` 为空时运行全部，否则只运行名称包含它的测试。
pub task teal_test(path: string, tier: string, filter: string,) -> { string: int | string } | Error {
    return toolchain_host::test(path, tier, filter).await ?;
}
pub task teal_run(path: string, tier: string, permission_file: string,) -> { string: int | string } | Error {
    return toolchain_host::run(path, tier, permission_file).await ?;
}
pub task fetch(url: string) -> string | Error {
    return web_host::fetch(url).await ?;
}

/// 读取工作区内 PNG/JPEG；返回原始 Base64 与尺寸摘要，Agent 桥另行附带图片。
pub task read_image(path: string) -> { string: string } | Error {
    return workspace_host::read_image(path).await ?;
}
