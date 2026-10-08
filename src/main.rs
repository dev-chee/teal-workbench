//! `teal` 总入口只把命令派发给同一发行目录中的独立工具。

mod dispatch;

fn main() {
    std::process::exit(dispatch::run(std::env::args_os().skip(1)));
}
