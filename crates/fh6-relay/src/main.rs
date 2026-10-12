//! FH6 遥测 UDP 分发器（relay）—— 命令行入口。
//!
//! # 为什么需要这个东西
//!
//! FH6 的「数据输出」**只支持一个目标地址 + 一个端口**（官方原文：
//! `A single packet format is sent to the configured address`），
//! 而飞智空间站的八爪鱼5 自适应扳机模块
//! （`mods/forza_trigger/ForzaDualSense.exe`）会**独占监听 `127.0.0.1:5300`**
//! —— 实测该进程启动后即绑定该端口，且其配置文件里没有端口项，用户改不了。
//!
//! Windows 的 UDP 端口共享语义（本机实测，Windows 10 19045）：
//!
//! | 场景 | 结果 |
//! |---|---|
//! | 两个套接字都设 `SO_REUSEADDR` 绑同一端口 | 都能 bind 成功，但**单播包只投递给第一个绑定者** |
//! | 一个设 `SO_REUSEADDR`、另一个不设 | 后绑定的 **bind 直接失败** |
//! | 都不设 | 后绑定的 bind 失败 |
//!
//! 所以**不可能**让 HUD 和飞智各自独立地同时收到同一份数据包。
//!
//! # 本方案怎么绕过去
//!
//! 关键突破口是：**游戏里的「数据输出IP地址」和「数据输出IP端口」都是可以
//! 自由填写的**，5300 只是飞智推荐的取值，不是游戏强制的。
//!
//! 因此**不去抢 5300**（抢了会把飞智顶掉），而是：
//!
//! ```text
//! FH6 ──Data Out──► 127.0.0.1:5301  (本 relay 独占监听)
//!                        │
//!          ┌─────────────┴─────────────┐
//!          ▼                           ▼
//!   127.0.0.1:5300                127.0.0.1:5302
//!   飞智 ForzaDualSense              fh6-hud
//!   （自适应扳机照常）              （本项目的 HUD）
//! ```
//!
//! 游戏内只需要把端口从 `5300` 改成 `5301`，飞智完全无感。
//! 想再接 SimHub / 录数据工具，加一个 `--to` 目标即可（天然支持任意多个）。

use std::env;
use std::net::{SocketAddr, ToSocketAddrs};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use fh6_relay::{parse_host_port, should_log, Rate, Relay};

/// 本 relay 默认监听的端口。
///
/// 选 5301 是因为飞智占着 5300；同时官方 FH6 文档特别提醒：
/// `Avoid ports 5200 through 5300, as the game binds its own outgoing socket
/// to a port in this range.` —— 5301 刚好避开这个「游戏自用」区间。
const DEFAULT_LISTEN_PORT: u16 = 5301;

/// 默认转发目标：飞智（5300）与 HUD（5302）。
///
/// 顺序有意把飞智放第一：万一将来需要按顺序降级，优先保证扳机不失效。
const DEFAULT_TARGETS: [(&str, u16); 2] = [
    ("127.0.0.1", fh6_core::FLYDIGI_FORZA_PORT),
    ("127.0.0.1", 5302),
];

/// 日志输出间隔。
const LOG_INTERVAL: Duration = Duration::from_secs(5);

/// 命令行配置。
struct Config {
    listen: SocketAddr,
    targets: Vec<(String, u16)>,
    verbose: bool,
}

const HELP: &str = "\
fh6-relay — FH6 遥测 UDP 分发器

用法:
    fh6-relay [选项]

选项:
    -l, --listen <IP:端口>    监听地址，默认 127.0.0.1:5301
    -t, --to <IP:端口>        追加一个转发目标（可重复）
                              默认转发到 127.0.0.1:5300（飞智）与 127.0.0.1:5302（HUD）
    -v, --verbose             打印每个数据包的简要信息（调试用，会显著增加 I/O）
    -h, --help                显示本帮助
    -V, --version             显示版本

游戏内设置（SETTINGS > HUD AND GAMEPLAY）:
    数据输出          = 开启
    数据输出IP地址    = 127.0.0.1
    数据输出IP端口    = 5301      <- 注意不是 5300

示例:
    # 默认：收 5301，转发给飞智(5300) 和 HUD(5302)
    fh6-relay

    # 再加一个 SimHub
    fh6-relay --to 127.0.0.1:8001

    # 换个监听端口
    fh6-relay --listen 127.0.0.1:5400
";

fn main() -> ExitCode {
    let config = match parse_args(env::args().skip(1)) {
        Ok(Some(cfg)) => cfg,
        Ok(None) => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Err(msg) => {
            eprintln!("参数错误: {msg}");
            eprintln!("用 --help 查看用法。");
            return ExitCode::from(2);
        }
    };

    match run(config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("启动失败: {e}");
            ExitCode::FAILURE
        }
    }
}

/// 解析命令行参数。返回 `Ok(None)` 表示「打印帮助/版本后正常退出」。
fn parse_args<I>(args: I) -> Result<Option<Config>, String>
where
    I: Iterator<Item = String>,
{
    let mut listen: Option<SocketAddr> = None;
    let mut targets: Vec<(String, u16)> = Vec::new();
    let mut verbose = false;

    let mut it = args;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "-V" | "--version" => {
                println!("fh6-relay {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "-v" | "--verbose" => verbose = true,
            "-l" | "--listen" => {
                let v = it.next().ok_or("--listen 需要一个地址参数")?;
                listen = Some(resolve_addr(&v)?);
            }
            "-t" | "--to" => {
                let v = it.next().ok_or("--to 需要一个地址参数")?;
                targets.push(parse_host_port(&v)?);
            }
            other => {
                // 支持 --listen=127.0.0.1:5301 这种等号写法
                if let Some(rest) = other.strip_prefix("--listen=") {
                    listen = Some(resolve_addr(rest)?);
                } else if let Some(rest) = other.strip_prefix("--to=") {
                    targets.push(parse_host_port(rest)?);
                } else {
                    return Err(format!("无法识别的参数 `{other}`"));
                }
            }
        }
    }

    if targets.is_empty() {
        targets = DEFAULT_TARGETS
            .iter()
            .map(|(h, p)| ((*h).to_string(), *p))
            .collect();
    }

    let listen = match listen {
        Some(a) => a,
        None => SocketAddr::from(([127, 0, 0, 1], DEFAULT_LISTEN_PORT)),
    };

    Ok(Some(Config {
        listen,
        targets,
        verbose,
    }))
}

/// 解析 `IP:端口` 或 `主机:端口` 为套接字地址。
fn resolve_addr(s: &str) -> Result<SocketAddr, String> {
    if let Ok(a) = s.parse::<SocketAddr>() {
        return Ok(a);
    }
    s.to_socket_addrs()
        .map_err(|e| format!("无法解析地址 `{s}`: {e}"))?
        .next()
        .ok_or_else(|| format!("地址 `{s}` 没有解析出任何结果"))
}

/// 主循环：绑定端口后持续收包并原样转发。
fn run(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let mut relay = Relay::bind(config.listen, &config.targets).map_err(|e| {
        format!(
            "无法绑定 {}: {e}\n\
             提示：该端口可能已被占用。可用 `netstat -ano -p UDP | findstr :{}` 查看。",
            config.listen,
            config.listen.port()
        )
    })?;

    let actual = relay.local_addr()?;

    println!(
        "fh6-relay {} —— FH6 遥测 UDP 分发器",
        env!("CARGO_PKG_VERSION")
    );
    println!("监听: {actual}");
    for (label, addr) in relay.targets() {
        println!("转发: {actual} -> {label}  ({addr})");
    }
    println!();
    println!("游戏内请设置（SETTINGS > HUD AND GAMEPLAY）:");
    println!("    数据输出       = 开启");
    println!("    数据输出IP地址 = {}", actual.ip());
    println!("    数据输出IP端口 = {}", actual.port());
    println!();
    println!("按 Ctrl+C 退出。");
    println!();

    let mut last_log = Instant::now();
    let verbose = config.verbose;
    let mut since_last_log = 0u64;

    relay.set_read_timeout(Some(Duration::from_millis(250)))?;

    loop {
        match relay.pump_once() {
            Ok(Some(len)) => {
                since_last_log += 1;
                if verbose {
                    println!("[{since_last_log}] 收到 {len} 字节");
                }
            }
            Ok(None) => {}
            Err(e) => return Err(Box::new(e)),
        }

        if should_log(last_log, LOG_INTERVAL) {
            let elapsed = last_log.elapsed();
            let stats = relay.stats();
            let rate = Rate::from(&stats, elapsed);
            let mut line = format!(
                "统计: {} 包 / {:.1}s ({:.1} 包/秒, {:.1} kbps)",
                stats.packets,
                elapsed.as_secs_f32(),
                rate.packets_per_sec,
                rate.kbps
            );
            for (label, ts) in relay.target_stats() {
                line.push_str(&format!(" | {label} 成功{} 失败{}", ts.sent, ts.failed));
            }
            if stats.short_packets > 0 {
                line.push_str(&format!(" | 短包{}", stats.short_packets));
            }
            if let Some(s) = stats.last_sender {
                line.push_str(&format!(" | 来源{s}"));
            }
            println!("{line}");
            last_log = Instant::now();
        }
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    #[test]
    fn 默认配置应监听5301并转发到5300与5302() {
        let cfg = parse_args(std::iter::empty()).unwrap().unwrap();
        assert_eq!(cfg.listen.port(), DEFAULT_LISTEN_PORT);
        assert_eq!(cfg.listen.port(), 5301);
        assert_eq!(cfg.targets.len(), 2);
        assert_eq!(cfg.targets[0], ("127.0.0.1".to_string(), 5300));
        assert_eq!(cfg.targets[1], ("127.0.0.1".to_string(), 5302));
        assert_eq!(
            cfg.targets[0].1,
            fh6_core::FLYDIGI_FORZA_PORT,
            "默认第一个目标必须是飞智的端口常量"
        );
    }

    #[test]
    fn 可以追加转发目标() {
        let args = vec!["--to".to_string(), "127.0.0.1:8001".to_string()];
        let cfg = parse_args(args.into_iter()).unwrap().unwrap();
        // 指定 --to 时不再使用默认目标
        assert_eq!(cfg.targets, vec![("127.0.0.1".to_string(), 8001)]);
    }

    #[test]
    fn 支持等号写法与多个目标() {
        let args = vec![
            "--listen=127.0.0.1:5400".to_string(),
            "--to=127.0.0.1:5300".to_string(),
            "--to=127.0.0.1:5302".to_string(),
            "--to=127.0.0.1:8001".to_string(),
        ];
        let cfg = parse_args(args.into_iter()).unwrap().unwrap();
        assert_eq!(cfg.listen.port(), 5400);
        assert_eq!(cfg.targets.len(), 3);
    }

    #[test]
    fn help与version应正常退出() {
        assert!(parse_args(vec!["--help".to_string()].into_iter())
            .unwrap()
            .is_none());
        assert!(parse_args(vec!["-V".to_string()].into_iter())
            .unwrap()
            .is_none());
    }

    #[test]
    fn 无法识别的参数报错() {
        assert!(parse_args(vec!["--nope".to_string()].into_iter()).is_err());
        assert!(parse_args(vec!["-l".to_string()].into_iter()).is_err());
    }

    #[test]
    fn 缺少端口时给出明确错误() {
        assert!(parse_host_port("127.0.0.1").is_err());
        assert!(parse_host_port("127.0.0.1:abc").is_err());
        let (h, p) = parse_host_port("127.0.0.1:5300").unwrap();
        assert_eq!(h, "127.0.0.1");
        assert_eq!(p, 5300);
    }

    #[test]
    fn 地址解析支持端口字面量() {
        assert_eq!(resolve_addr("127.0.0.1:5301").unwrap().port(), 5301);
        // localhost 需要 DNS，大多数环境可用
        if let Ok(a) = resolve_addr("localhost:5301") {
            assert_eq!(a.port(), 5301);
        }
    }
}
