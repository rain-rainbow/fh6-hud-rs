//! FH6 轻量原生 HUD 叠加层 —— 当前为骨架，渲染层待实现。
//!
//! 设计目标（对比参考实现的 PyQt6 方案）：
//!
//! | 指标 | 参考实现 (Python + PyQt6) | 本项目 (Rust + 原生窗口) |
//! |---|---|---|
//! | 分发体积 | 解包后 150–250 MB | 目标 1–3 MB 单文件 |
//! | 内存 | 数百 MB | 目标 15–30 MB |
//! | 每帧渲染 | 全屏 CPU 光栅化 + 全屏分层窗口 | 脏区重绘 + GPU 合成 |
//!
//! 已确定的关键实现要点（见 `docs/架构决策.md`）：
//!
//! 1. **窗口样式**：`WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT`。
//!    最后一项 `WS_EX_TRANSPARENT` 让鼠标事件穿透到游戏 —— 参考实现**没有**做
//!    点击穿透，会挡住游戏操作，这是本项目要修正的点。
//! 2. **不阻塞收包**：UDP 接收放在独立线程，渲染线程只读最新快照。
//!    参考实现把收包、业务、渲染全挤在 Qt 主线程，掉帧时会丢遥测包
//!    （UDP 无重传，丢了就永久丢了）。
//! 3. **只在数据或动画变化时重绘**，静止时降到低频。
//!
//! 当前仅提供命令行信息与配置解析，渲染层随后续步骤实现。

use std::env;
use std::process::ExitCode;

/// HUD 默认监听的端口，对应 [`fh6_core::FLYDIGI_FORZA_PORT`] 之外的第二个转发目标。
const DEFAULT_HUD_PORT: u16 = 5302;

const HELP: &str = "\
fh6-hud — FH6 轻量原生 HUD 叠加层

用法:
    fh6-hud [选项]

选项:
    -l, --listen <IP:端口>    遥测来源地址，默认 127.0.0.1:5302
    -h, --help                显示本帮助
    -V, --version             显示版本

配套使用:
    先运行 fh6-relay 做 UDP 分发（游戏发 5301，relay 转发给飞智 5300 与本 HUD 5302）,
    再运行本程序。游戏内「数据输出IP端口」填 5301。

当前版本说明:
    渲染层尚未实现。本版本仅完成 crate 骨架、配置解析与命令行接口，
    渲染实现随后续步骤补齐。
";

fn main() -> ExitCode {
    let mut listen_port = DEFAULT_HUD_PORT;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("fh6-hud {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            "-l" | "--listen" => {
                let Some(v) = args.next() else {
                    eprintln!("参数错误: --listen 需要一个地址参数");
                    return ExitCode::from(2);
                };
                match parse_port(&v) {
                    Ok(p) => listen_port = p,
                    Err(msg) => {
                        eprintln!("参数错误: {msg}");
                        return ExitCode::from(2);
                    }
                }
            }
            other => {
                if let Some(rest) = other.strip_prefix("--listen=") {
                    match parse_port(rest) {
                        Ok(p) => listen_port = p,
                        Err(msg) => {
                            eprintln!("参数错误: {msg}");
                            return ExitCode::from(2);
                        }
                    }
                } else {
                    eprintln!("参数错误: 无法识别的参数 `{other}`");
                    eprintln!("用 --help 查看用法。");
                    return ExitCode::from(2);
                }
            }
        }
    }

    println!("fh6-hud {} —— FH6 轻量原生 HUD", env!("CARGO_PKG_VERSION"));
    println!("遥测来源: 127.0.0.1:{listen_port}");
    println!();
    println!("⚠ 渲染层尚未实现（当前为骨架版本）。");
    println!("  已完成: 数据包解析 (fh6-core)、UDP 分发器 (fh6-relay)、工程骨架。");
    println!("  待完成: 透明置顶窗口、仪表渲染、漂移状态机。");

    ExitCode::SUCCESS
}

/// 解析端口号，允许传入 `127.0.0.1:5302` 或纯端口 `5302` 两种形式。
fn parse_port(s: &str) -> Result<u16, String> {
    let s = s.trim();
    // 先尝试纯数字
    if let Ok(p) = s.parse::<u16>() {
        return Ok(p);
    }
    // 再尝试 IP:端口，取最后一个冒号后的部分
    if let Some(idx) = s.rfind(':') {
        return s[idx + 1..]
            .parse::<u16>()
            .map_err(|e| format!("`{s}` 的端口不合法: {e}"));
    }
    Err(format!(
        "`{s}` 不是合法的端口，正确写法如 5302 或 127.0.0.1:5302"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 默认端口是5302() {
        assert_eq!(DEFAULT_HUD_PORT, 5302);
    }

    #[test]
    fn 端口解析支持两种写法() {
        assert_eq!(parse_port("5302").unwrap(), 5302);
        assert_eq!(parse_port("127.0.0.1:5302").unwrap(), 5302);
        assert_eq!(parse_port("  5400  ").unwrap(), 5400);
    }

    #[test]
    fn 非法端口报错() {
        assert!(parse_port("abc").is_err());
        assert!(parse_port("70000").is_err());
        assert!(parse_port("").is_err());
    }
}
