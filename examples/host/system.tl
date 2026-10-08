@edition("2026");

// Tier Host：系统信息只暴露宿主明确实现的值；环境变量仍需名称级资源授权。
import "std/system.tl" as system;

task main() {
    print(system::platform() != "");
    // 当前可执行文件既要求 process.inspect，也要求其路径位于可执行资源范围。
    print(system::current_executable() != "");
    print(system::utc_now() != "");
    print(system::monotonic_ms().is_int());

    // 示例只读取专用变量，不读取认证凭证或其他进程秘密。
    let setting = system::environment("TEAL_EXAMPLE_VALUE");
    print(setting.found);
    if setting.found { print(setting.value); }
}
