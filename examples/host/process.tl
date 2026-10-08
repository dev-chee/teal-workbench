@edition("2026");

// Tier Host：进程 API 只执行显式 argv，不经过 shell。
// `--allow-execute` 必须精确授权当前 Teal 可执行文件；进程 token 只能操作本次宿主启动的子进程。
import "std/process.tl" as process;
import "std/system.tl" as system;
import "std/error.tl" as errors;

task main() { workflow().await; }

task workflow() {
    // 在 teal-run sibling 内运行：直接启动自身，不依赖 PowerShell、cmd 或 sh。
    let executable = system::current_executable();
    let resolved = process::resolve_or_throw(executable).await;
    print(resolved.found, resolved.path != "");
    print(process::find_by_executable_or_throw(executable).await.len() > 0);
    let command = process::command(executable, ["--code", "task main() {{ print(42); }}"]);
    let result = process::run_or_throw(command, 10000).await;
    match result.state {
        process::ProcessState.exited { code } => { print(code, result.stdout.trim()); }
        _ => { throw(errors::make("process", "Child process did not exit normally", errors::ErrorOrigin.host, false)); }
    }
    let handle = process::spawn_or_throw(command).await;
    let final = process::wait_or_throw(handle, 10000).await;
    match final.state {
        process::ProcessState.exited { code } => { print(code, final.stdout.trim()); }
        _ => { throw(errors::make("process", "Child process did not exit normally", errors::ErrorOrigin.host, false)); }
    }

    // 长任务避免 status 与自然退出竞速；查询后再用同一受管 token 终止。
    let sleeper = process::command(executable, ["--code", "fn main() {{ loop {{}} }}"]);
    let pending = process::spawn_or_throw(sleeper).await;
    match process::status_or_throw(pending).await.state {
        process::ProcessState.running => { print("running"); }
        _ => { throw(errors::make("process", "Child process did not remain running", errors::ErrorOrigin.host, false)); }
    }
    let terminated = process::terminate_or_throw(pending).await;
    match terminated.state {
        process::ProcessState.terminated => { print("terminated"); }
        _ => { throw(errors::make("process", "Child process was not terminated", errors::ErrorOrigin.host, false)); }
    }
}
