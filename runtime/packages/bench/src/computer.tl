@edition("2026");

import "std/json.tl" as json;

host "std.bench.computer" as backend {
    task list_windows() -> string[] | Error;

    task snapshot(process_id: int, window_handle: string, generation: int) -> string | Error;

    task invoke(process_id: int, window_handle: string, generation: int, runtime_id: string) -> none | Error;

    task set_value(process_id: int, window_handle: string, generation: int, runtime_id: string, value: string) -> none | Error;

    task raw_click(process_id: int, window_handle: string, generation: int, runtime_id: string) -> none | Error;

    task raw_keys(process_id: int, window_handle: string, generation: int, runtime_id: string, keys: string) -> none | Error;
}

/// 带代次约束的桌面顶层窗口身份。
pub class ComputerWindow {
    process_id: int,
    window_handle: string,
    title: string,
    generation: int,
}

impl ComputerWindow {
    /// 返回窗口所属进程标识。
    pub fn process_id() -> int {
        return self.process_id;
    }

    /// 返回窗口标题。
    pub fn title() -> string {
        return self.title;
    }

    /// 返回只适用于当前窗口状态的可访问性代次。
    pub fn generation() -> int {
        return self.generation;
    }
}

/// 可访问性树 JSON 及其更新后的窗口代次。
pub class ComputerSnapshot {
    pub window: ComputerWindow,
    pub json: string,
}

/// 解码 `list_windows` 返回的单个窗口 JSON。
pub fn decode_window(data: string) -> ComputerWindow {
    return ComputerWindow {
        process_id: int(json::read_number(data, "/process_id")),
        window_handle: json::read_string(data, "/window_handle"),
        title: json::read_string(data, "/title"),
        generation: int(json::read_number(data, "/generation")),
    };
}

/// 列出可见顶层窗口；每项可交给 `decode_window`。
pub task list_windows() -> string[] | Error {
    return backend::list_windows().await ?;
}

/// 读取有界 UI Automation 树，并返回只适用于该树的新代次。
pub task snapshot(window: ComputerWindow) -> ComputerSnapshot | Error {
    let data = backend::snapshot(window.process_id, window.window_handle, window.generation,).await ?;
    return ComputerSnapshot {
        window: ComputerWindow {
            process_id: int(json::read_number(data, "/window/process_id")),
            window_handle: json::read_string(data, "/window/window_handle"),
            title: json::read_string(data, "/window/title"),
            generation: int(json::read_number(data, "/window/generation")),
        },
        json: data,
    };
}

/// 通过 Invoke pattern 执行语义动作；调用后旧代次失效。
pub task invoke(window: ComputerWindow, runtime_id: string) -> none | Error {
    backend::invoke(window.process_id, window.window_handle, window.generation, runtime_id,).await ?;
}

/// 通过 Value pattern 写入非密码元素；调用后旧代次失效。
pub task set_value(window: ComputerWindow, runtime_id: string, value: string) -> none | Error {
    backend::set_value(window.process_id, window.window_handle, window.generation, runtime_id, value,).await ?;
}

/// 经独立原始输入能力点击非密码元素中心；调用后旧代次失效。
pub task raw_click(window: ComputerWindow, runtime_id: string) -> none | Error {
    backend::raw_click(window.process_id, window.window_handle, window.generation, runtime_id,).await ?;
}

/// 经独立原始输入能力向非密码元素发送按键；调用后旧代次失效。
pub task raw_keys(window: ComputerWindow, runtime_id: string, keys: string) -> none | Error {
    backend::raw_keys(window.process_id, window.window_handle, window.generation, runtime_id, keys,).await ?;
}
