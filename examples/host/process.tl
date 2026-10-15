@edition("2026");

// Tier Host：进程 API 只执行显式 argv，不经过 shell。
// `--allow-execute` 必须精确授权当前 Teal 可执行文件；进程 token 只能操作本次宿主启动的子进程。
import "std/process.tl" as process;
import "std/system.tl" as system;
task application() -> none | Error {
    workflow().await ?;
}

task workflow() -> none | Error {
    // 在 teal-run sibling 内运行：直接启动自身，不依赖 PowerShell、cmd 或 sh。
    let executable = system::current_executable();
    let resolved = process::resolve(executable).await ?;
    print(resolved.found, resolved.path != "");
    print(process::find_by_executable(executable).await ?.len() > 0);
    let command = process::command(executable, ["--code", "task main() {{ print(42); }}"]);
    let result = process::run(command, 10000).await ?;
    match result.state {
        process::ProcessState.exited { code } => {
            print(code, result.stdout.trim());
        } _ => {
            return Error::make("process", "Child process did not exit normally", ErrorOrigin. host, false);
        }
    }
    let handle = process::spawn(command).await ?;
    let final = process::wait(handle, 10000).await ?;
    match final.state {
        process::ProcessState.exited { code } => {
            print(code, final.stdout.trim());
        } _ => {
            return Error::make("process", "Child process did not exit normally", ErrorOrigin. host, false);
        }
    }

    // 长任务避免 status 与自然退出竞速；查询后再用同一受管 token 终止。
    let sleeper = process::command(executable, ["--code", "fn main() {{ loop {{}} }}"]);
    let pending = process::spawn(sleeper).await ?;
    match process::status(pending).await ?.state {
        process::ProcessState.running => {
            print("running");
        } _ => {
            return Error::make("process", "Child process did not remain running", ErrorOrigin. host, false);
        }
    }
    let terminated = process::terminate(pending).await ?;
    match terminated.state {
        process::ProcessState.terminated => {
            print("terminated");
        } _ => {
            return Error::make("process", "Child process was not terminated", ErrorOrigin. host, false);
        }
    }
}

/// 入口明确处理失败；未知外部效果仍由 VM 终止。
task main() -> int {
    let result = application().await;
    match result {
        Error { .. } => {
            print(result.message);
            return 1;
        } _ => {
            return 0;
        }
    }
}
