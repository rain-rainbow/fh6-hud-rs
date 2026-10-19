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

use fh6_config::AppConfig;
use fh6_relay::{parse_host_port, should_log, Rate, Relay};

/// 日志输出间隔。
const LOG_INTERVAL: Duration = Duration::from_secs(5);

/// 命令行配置。
///
/// 所有字段都是 `Option`/可为空：解析阶段**不填默认值**，目的是把
/// 「命令行是否显式指定」这件事保留到与配置文件合并的那一刻。
/// 合并优先级：命令行 > 配置文件 > 内置默认值。
#[derive(Debug, Default)]
struct CliConfig {
    config_file: Option<String>,
    listen: Option<SocketAddr>,
    targets: Vec<(String, u16)>,
    verbose: Option<bool>,
}

const HELP: &str = "\
fh6-relay — FH6 遥测 UDP 分发器

用法:
    fh6-relay [选项]

选项:
    -c, --config <文件>       读取完整配置（与 GUI 共用同一份 AppConfig）
    -l, --listen <IP:端口>    监听地址，默认取配置或 127.0.0.1:5301
    -t, --to <IP:端口>        追加一个转发目标（可重复）
                              未指定时用配置里的目标；配置也没有则用
                              127.0.0.1:5300（飞智）与 127.0.0.1:5302（HUD）
    -v, --verbose             打印每个数据包的简要信息（调试用，会显著增加 I/O）
    -h, --help                显示本帮助
    -V, --version             显示版本

说明:
    分发目标的地址、端口、数量都可以在配置文件里任意增删，
    也可以用 --to 临时追加/覆盖。用 `--to` 指定时不再使用配置文件里的目标。

游戏内设置（SETTINGS > HUD AND GAMEPLAY）:
    数据输出          = 开启
    数据输出IP地址    = 127.0.0.1
    数据输出IP端口    = 5301      <- 注意不是 5300

示例:
    # 默认：收 5301，转发给飞智(5300) 和 HUD(5302)
    fh6-relay

    # 用配置文件（GUI 生成的那份）
    fh6-relay --config fh6-hud.config.json

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
fn parse_args<I>(args: I) -> Result<Option<CliConfig>, String>
where
    I: Iterator<Item = String>,
{
    let mut cfg = CliConfig::default();

    let mut it = args;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "-V" | "--version" => {
                println!("fh6-relay {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "-v" | "--verbose" => cfg.verbose = Some(true),
            "-c" | "--config" => {
                let v = it.next().ok_or("--config 需要一个文件路径")?;
                cfg.config_file = Some(v);
            }
            "-l" | "--listen" => {
                let v = it.next().ok_or("--listen 需要一个地址参数")?;
                cfg.listen = Some(resolve_addr(&v)?);
            }
            "-t" | "--to" => {
                let v = it.next().ok_or("--to 需要一个地址参数")?;
                cfg.targets.push(parse_host_port(&v)?);
            }
            other => {
                // 支持 --listen=127.0.0.1:5301 这种等号写法
                if let Some(rest) = other.strip_prefix("--listen=") {
                    cfg.listen = Some(resolve_addr(rest)?);
                } else if let Some(rest) = other.strip_prefix("--to=") {
                    cfg.targets.push(parse_host_port(rest)?);
                } else if let Some(rest) = other.strip_prefix("--config=") {
                    cfg.config_file = Some(rest.to_string());
                } else {
                    return Err(format!("无法识别的参数 `{other}`"));
                }
            }
        }
    }

    Ok(Some(cfg))
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
///
/// 配置来源合并顺序（后者覆盖前者）：
/// 内置默认值 → 配置文件中的 `relay` 段 → 命令行参数。
fn run(cli: CliConfig) -> Result<(), Box<dyn std::error::Error>> {
    // ---- 1. 读配置文件 ----
    // 配置来源优先级：--config 显式指定 > exe 同目录的默认文件 > 内置默认值。
    // 自动发现让「GUI 里存好的配置」对直接启动的 relay 同样生效。
    let auto_path = AppConfig::default_path();
    let (mut app, note) = match &cli.config_file {
        Some(p) => AppConfig::load(std::path::Path::new(p)),
        None if auto_path.exists() => AppConfig::load(&auto_path),
        None => (AppConfig::default(), None),
    };
    if let Some(n) = note {
        eprintln!("配置: {n}");
    } else if let Some(p) = &cli.config_file {
        println!("配置: 已加载 {p}");
    } else if auto_path.exists() {
        println!("配置: 已自动加载 {}", auto_path.display());
    }

    // ---- 2. 命令行覆盖 ----
    if let Some(a) = cli.listen {
        app.relay.listen_ip = a.ip().to_string();
        app.relay.listen_port = a.port();
    }
    if !cli.targets.is_empty() {
        // 显式给了 --to 就完全替换配置文件里的目标列表
        app.relay.targets = cli
            .targets
            .iter()
            .map(|(h, p)| fh6_config::RelayTarget::new(h, *p, ""))
            .collect();
    }
    if let Some(v) = cli.verbose {
        app.relay.verbose = v;
    }
    let fixed = app.relay.sanitize();
    if fixed {
        eprintln!("[!] relay 配置有项被修正（自转发目标已禁用 / 无效目标已剔除）");
    }
    if let Some(p) = &cli.config_file {
        // 配置里若声明了日志，就用它
        let _ = p;
    }

    // ---- 3. 初始化日志 ----
    app.init_logging("fh6-relay");
    let log = fh6_config::Logger::global();

    let listen_ip = app.relay.listen_ip.clone();
    let listen_port = app.relay.listen_port;
    let listen = resolve_addr(&format!("{listen_ip}:{listen_port}"))
        .map_err(|e| format!("监听地址无法解析: {e}"))?;
    let targets = app.relay.enabled_targets();
    let verbose = app.relay.verbose;

    if targets.is_empty() {
        return Err("没有任何启用的分发目标：请在配置文件里至少启用一个目标".into());
    }

    let mut relay = Relay::bind(listen, &targets).map_err(|e| {
        format!(
            "无法绑定 {listen}: {e}\n\
             提示：该端口可能已被占用。可用 `netstat -ano -p UDP | findstr :{}` 查看。",
            listen.port()
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
    println!("目标数量: {}", targets.len());
    if app.logging.enabled {
        println!("日志文件: {}", app.logging.path);
    }
    println!();
    println!("游戏内请设置（SETTINGS > HUD AND GAMEPLAY）:");
    println!("    数据输出       = 开启");
    println!("    数据输出IP地址 = {}", actual.ip());
    println!("    数据输出IP端口 = {}", actual.port());
    println!();
    println!("按 Ctrl+C 退出。");
    println!();

    log.info(&format!(
        "relay 已绑定 {actual}，{} 个目标: {}",
        targets.len(),
        targets
            .iter()
            .map(|(h, p)| format!("{h}:{p}"))
            .collect::<Vec<_>>()
            .join(", ")
    ));

    let mut last_log = Instant::now();
    let mut since_last_log = 0u64;

    relay.set_read_timeout(Some(Duration::from_millis(250)))?;

    loop {
        match relay.pump_once() {
            Ok(Some(len)) => {
                since_last_log += 1;
                if verbose {
                    println!("[{since_last_log}] 收到 {len} 字节");
                }
                log.debug(&format!("收到 {len} 字节"));
            }
            Ok(None) => {}
            Err(e) => {
                log.error(&format!("收包失败: {e}"));
                return Err(Box::new(e));
            }
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
            log.info(&line);
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
        // 命令行不带参数时，解析结果里不应有「显式指定」的项，
        // 真正的默认值来自 AppConfig（配置文件缺字段时也取它）。
        let cli = parse_args(std::iter::empty()).unwrap().unwrap();
        assert!(cli.listen.is_none(), "未指定 --listen 时应为 None");
        assert!(cli.targets.is_empty(), "未指定 --to 时不应有目标");
        assert!(cli.verbose.is_none());

        let app = AppConfig::default();
        assert_eq!(app.relay.listen_port, 5301);
        assert_eq!(app.relay.listen_ip, "127.0.0.1");
        let t = app.relay.enabled_targets();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0], ("127.0.0.1".to_string(), 5300));
        assert_eq!(t[1], ("127.0.0.1".to_string(), 5302));
        assert_eq!(
            t[0].1,
            fh6_core::FLYDIGI_FORZA_PORT,
            "默认第一个目标必须是飞智的端口常量"
        );
    }

    #[test]
    fn 支持配置文件参数两种写法() {
        let a = parse_args(vec!["--config".to_string(), "x.json".to_string()].into_iter())
            .unwrap()
            .unwrap();
        assert_eq!(a.config_file.as_deref(), Some("x.json"));
        let b = parse_args(vec!["--config=x.json".to_string()].into_iter())
            .unwrap()
            .unwrap();
        assert_eq!(b.config_file.as_deref(), Some("x.json"));
        assert!(parse_args(vec!["--config".to_string()].into_iter()).is_err());
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
        assert_eq!(cfg.listen.map(|a| a.port()), Some(5400));
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
