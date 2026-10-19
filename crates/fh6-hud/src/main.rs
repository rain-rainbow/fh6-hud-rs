//! FH6 轻量原生 HUD 叠加层 —— 程序入口。
//!
//! 提供两种运行模式：
//!
//! | 模式 | 参数 | 用途 |
//! |---|---|---|
//! | 实时模式 | （默认） | 创建透明置顶窗口，收 UDP 遥测并实时绘制 |
//! | 自检模式 | `--selftest` | **不创建窗口**，离屏渲染一帧存成 BMP，用于核对绘制 |
//!
//! # 为什么要单独做自检模式
//!
//! 渲染层无法用单元测试验证「看起来对不对」。但把绘制代码放进一个真的窗口里
//! 检查，代价很高：窗口会盖住屏幕、需要 GPU、出问题时要靠肉眼盯着看。
//!
//! 自检模式把同一套绘制代码跑在一块**内存画布**上，然后写成 BMP。
//! 好处是：不需要窗口、结果是一个可以事后比对的文件、出问题容易定位。
//!
//! # 架构：为什么收包与渲染分线程
//!
//! 参考实现把 UDP 轮询、业务逻辑、渲染全部塞在 Qt 主线程的一个 16 ms 定时器里。
//! 渲染一旦超时，就来不及排空 socket，遥测包被静默丢弃 —— 而 UDP 无重传。
//!
//! 本实现：
//!
//! ```text
//!   [收包线程]  recv_from 阻塞 ──► 解析 ──► 写入共享快照
//!                                              │
//!   [渲染线程]  按帧率读取快照 ◄────────────────┘
//!               ▼
//!          离屏画布 ──► UpdateLayeredWindow（只提交脏区）
//! ```
//!
//! 收包线程只做「收 + 解析 + 写快照」，**永不渲染**；渲染卡住也不会丢包。

use std::env;
use std::fs::File;
use std::io::Write;
use std::net::UdpSocket;
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use fh6_core::drift::{DriftJudge, DriftSnapshot};
use fh6_core::{derived, Frame, Telemetry};

use fh6_config::settings::Settings;
use fh6_config::{AppConfig, Language};
use fh6_hud::gauge::{self, Bgra, DirtyRegion, Rect, Theme, TrailBuf};
use fh6_hud::shared::{LinkState, StopFlag, TelemetrySlot};
use fh6_hud::timing::{FrameClock, MovingAverage};
use fh6_hud::window::{Canvas, LayeredWindow, WindowOptions};
use fh6_hud::{HudFrame, HudPainter};

/// 收包缓冲区。数据包 324 字节，留足余量。
const RECV_BUF: usize = 2048;

const HELP: &str = "\
fh6-hud — FH6 轻量原生 HUD 叠加层

用法:
    fh6-hud [选项]

选项:
    -l, --listen <地址:端口>  遥测来源，默认 127.0.0.1:5302
    -c, --config <文件>      从 JSON 加载外观参数（不存在则用默认值）
        --save-config <文件> 退出时把当前参数写入该文件
        --selftest [文件]    不创建窗口，离屏渲染一帧存为 BMP 后退出
                             默认输出 hud-selftest.bmp
        --size <宽x高>       画布/窗口尺寸，默认取虚拟屏幕大小
        --no-topmost         不置顶（调试用）
        --capture <文件>     接收若干帧后把真实绘制帧存成 BMP 并退出
        --diag               诊断模式：打印实测窗口/画布尺寸，并在四角画标记
    -h, --help               显示本帮助
    -V, --version            显示版本

配套使用:
    1. 先运行 fh6-relay（游戏发 5301，relay 转发给飞智 5300 与本 HUD 5302）
    2. 再运行本程序
    3. 游戏内「数据输出IP端口」填 5301

热键（HUD 拿到焦点时）:
    Esc   退出
";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match parse_args(&args) {
        Ok(Action::Help) => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Ok(Action::Version) => {
            println!("fh6-hud {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(Action::SelfTest(cfg)) => match run_selftest(&cfg) {
            Ok(path) => {
                println!("自检完成，已写出: {path}");
                println!("用图片查看器打开它即可核对绘制是否正确。");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("自检失败: {e}");
                ExitCode::FAILURE
            }
        },
        Ok(Action::Capture(cfg)) => match run_capture(&cfg) {
            Ok(path) => {
                println!("已把真实绘制帧写出到: {path}");
                println!("这是 HUD 真正上传到屏幕的那份像素，可用它离线核对几何与颜色。");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("抓帧失败: {e}");
                ExitCode::FAILURE
            }
        },
        Ok(Action::Run(cfg)) => match run_live(&cfg) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("运行失败: {e}");
                ExitCode::FAILURE
            }
        },
        Err(msg) => {
            eprintln!("参数错误: {msg}");
            eprintln!("用 --help 查看用法。");
            ExitCode::from(2)
        }
    }
}

/// 运行配置。
#[derive(Debug, Clone, Default)]
struct Config {
    /// 命令行显式指定的监听端点；`None` 表示取配置文件里的 `hud.network`。
    listen: Option<String>,
    config_file: Option<String>,
    save_config: Option<String>,
    selftest_out: Option<String>,
    /// `--capture <文件>`：接收若干帧后，把**真实绘制的那一帧**存成 BMP 并退出。
    ///
    /// 与 `--selftest` 的区别：selftest 用的是内置演示数据，capture 用的是
    /// **实际收到的遥测**（没有数据时就是默认零值），且走的是与实时模式
    /// 完全相同的绘制路径。用于把「屏幕上看到的」与「程序实际画的」区分开。
    capture_out: Option<String>,
    size: Option<(i32, i32)>,
    no_topmost: bool,
    /// 诊断模式：打印实测尺寸并在四角画标记，用于核对窗口是否 1:1 映射。
    diag: bool,
    /// 命令行覆盖界面语言（`zh` / `en`）。
    lang: Option<String>,
}

/// 解析后的动作。
enum Action {
    Help,
    Version,
    SelfTest(Config),
    Capture(Config),
    Run(Config),
}

fn parse_args(args: &[String]) -> Result<Action, String> {
    let mut cfg = Config::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => return Ok(Action::Help),
            "-V" | "--version" => return Ok(Action::Version),
            "-l" | "--listen" => {
                i += 1;
                cfg.listen = Some(args.get(i).ok_or("--listen 需要一个地址")?.clone());
            }
            "-c" | "--config" => {
                i += 1;
                cfg.config_file = Some(args.get(i).ok_or("--config 需要一个文件路径")?.clone());
            }
            "--save-config" => {
                i += 1;
                cfg.save_config =
                    Some(args.get(i).ok_or("--save-config 需要一个文件路径")?.clone());
            }
            "--selftest" => {
                // 可选输出路径：下一个参数不以 '-' 开头就当作文件名
                if let Some(p) = args.get(i + 1) {
                    if !p.starts_with('-') {
                        cfg.selftest_out = Some(p.clone());
                        i += 1;
                    }
                }
                if cfg.selftest_out.is_none() {
                    cfg.selftest_out = Some("hud-selftest.bmp".to_string());
                }
            }
            "--capture" => {
                i += 1;
                cfg.capture_out =
                    Some(args.get(i).ok_or("--capture 需要一个输出文件路径")?.clone());
            }
            "--size" => {
                i += 1;
                let v = args.get(i).ok_or("--size 需要 宽x高")?;
                cfg.size = Some(parse_size(v)?);
            }
            "--no-topmost" => cfg.no_topmost = true,
            "--diag" => cfg.diag = true,
            "--lang" => {
                i += 1;
                cfg.lang = Some(args.get(i).ok_or("--lang 需要 zh 或 en")?.clone());
            }
            other => {
                if let Some(v) = other.strip_prefix("--listen=") {
                    cfg.listen = Some(v.to_string());
                } else if let Some(v) = other.strip_prefix("--lang=") {
                    cfg.lang = Some(v.to_string());
                } else if let Some(v) = other.strip_prefix("--config=") {
                    cfg.config_file = Some(v.to_string());
                } else if let Some(v) = other.strip_prefix("--save-config=") {
                    cfg.save_config = Some(v.to_string());
                } else if let Some(v) = other.strip_prefix("--size=") {
                    cfg.size = Some(parse_size(v)?);
                } else if let Some(v) = other.strip_prefix("--selftest=") {
                    cfg.selftest_out = Some(v.to_string());
                } else {
                    return Err(format!("无法识别的参数 `{other}`"));
                }
            }
        }
        i += 1;
    }
    Ok(if cfg.selftest_out.is_some() {
        Action::SelfTest(cfg)
    } else if cfg.capture_out.is_some() {
        Action::Capture(cfg)
    } else {
        Action::Run(cfg)
    })
}

/// 解析 `宽x高`，同时接受 `1920x1080` 与 `1920*1080`。
fn parse_size(s: &str) -> Result<(i32, i32), String> {
    let norm = s.replace('*', "x").to_lowercase();
    let (a, b) = norm
        .split_once('x')
        .ok_or_else(|| format!("`{s}` 不是 宽x高 格式，正确写法如 1920x1080"))?;
    let w: i32 = a.trim().parse().map_err(|e| format!("宽度不合法: {e}"))?;
    let h: i32 = b.trim().parse().map_err(|e| format!("高度不合法: {e}"))?;
    if w < 64 || h < 64 {
        return Err(format!("尺寸过小: {w}x{h}，至少 64x64"));
    }
    Ok((w, h))
}

/// 加载设置。**容错**：文件不存在或损坏都用默认值，只打印提示，绝不因此退出。
/// 拆分 `地址:端口`。
///
/// 支持 `127.0.0.1:5302`、`0.0.0.0:5302`；也接受缺省地址的 `:5302`。
fn split_host_port(s: &str) -> Option<(String, u16)> {
    let s = s.trim();
    let (ip, port) = match s.rsplit_once(':') {
        Some(v) => v,
        // 只给了端口号
        None => ("127.0.0.1", s),
    };
    let port: u16 = port.parse().ok()?;
    let ip = if ip.trim().is_empty() {
        "127.0.0.1".to_string()
    } else {
        ip.trim().to_string()
    };
    Some((ip, port))
}

/// 加载完整配置（HUD + relay + 日志），并应用命令行覆盖。
///
/// 优先级：**命令行 > 配置文件 > 内置默认值**。
///
/// 配置文件里的 `relay` 部分本进程不直接用（那是 `fh6-relay` 的），
/// 但保留在 `AppConfig` 里整体读写，避免 HUD 的保存动作把 relay 配置抹掉。
fn load_app_config(cfg: &Config) -> AppConfig {
    let (mut app, note) = match &cfg.config_file {
        Some(path) => AppConfig::load(std::path::Path::new(path)),
        None => (AppConfig::default(), None),
    };
    if let Some(n) = note {
        println!("配置: {n}");
    } else if let Some(p) = &cfg.config_file {
        println!("配置: 已加载 {p}");
    }
    // 命令行覆盖监听端点
    if let Some(l) = &cfg.listen {
        match split_host_port(l) {
            Some((ip, port)) => {
                app.hud.network.listen_ip = ip;
                app.hud.network.listen_port = port;
            }
            None => println!("[!] 无法解析 --listen `{l}`，改用配置里的端点"),
        }
    }
    // 命令行覆盖语言
    if let Some(l) = &cfg.lang {
        app.hud.language = Language::parse(l);
    }
    app.sanitize();
    app
}

/// 实际监听的端点：命令行优先，否则用配置里的。
fn listen_endpoint(cfg: &Config, app: &AppConfig) -> String {
    cfg.listen
        .clone()
        .unwrap_or_else(|| app.hud.network.endpoint())
}

/// 构造一帧用于自检的示例数据（不需要真的连游戏）。
fn demo_hud_frame() -> (Frame, DriftSnapshot) {
    let mut judge = DriftJudge::new();

    // 模拟一个「右打方向、约 30° 漂移角」的稳定状态，让所有面板都有内容
    let telemetry = Telemetry {
        is_race_on: true,
        timestamp_ms: 123_456,
        current_engine_rpm: 7100.0,
        engine_max_rpm: 8000.0,
        engine_idle_rpm: 900.0,
        speed_mps: 33.3,                           // 约 120 km/h
        velocity: [33.3 * 0.5, 0.0, 33.3 * 0.866], // 约 30°
        acceleration: [9.0, 0.4, -3.0],
        gear_raw: 4,
        accel_raw: 204,
        brake_raw: 20,
        handbrake_raw: 10,
        steer_raw: 88,
        wheel_rl: fh6_core::Wheel {
            combined_slip: 1.42,
            ..Default::default()
        },
        wheel_rr: fh6_core::Wheel {
            combined_slip: 1.18,
            ..Default::default()
        },
        position: [2540.0, 441.0, 4918.0],
        current_lap_s: 42.7,
        ..Default::default()
    };

    // 跑若干帧让判定器的平滑量与状态机进入稳态，这样自检图才反映真实外观
    let d = derived::derive(&telemetry);
    let mut snap = DriftSnapshot::default();
    for _ in 0..180 {
        snap = judge.update(&telemetry, &d, 1.0 / 60.0);
    }
    (Frame::new(telemetry), snap)
}

/// 自检模式：离屏渲染并存成 BMP。
fn run_selftest(cfg: &Config) -> Result<String, String> {
    let app = load_app_config(cfg);
    let settings = app.hud.clone();
    let (cw, ch) = cfg.size.unwrap_or((1920, 1080));
    let mut canvas = Canvas::new(cw, ch)?;
    canvas.clear_transparent();

    let (core, drift) = demo_hud_frame();
    let hud = HudFrame::new(core, drift, LinkState::Live, 60.0, 1);

    // G 值轨迹：造一段弧线，验证轨迹绘制
    let mut trail: TrailBuf<{ gauge::G_TRAIL_LEN }> = TrailBuf::new();
    for i in 0..gauge::G_TRAIL_LEN {
        let t = i as f32 / gauge::G_TRAIL_LEN as f32;
        trail.push((t * 1.4 - 0.7, 0.7 * (1.0 - t)));
    }
    let trail_pts: Vec<(f32, f32)> = trail.iter().collect();

    {
        let mut painter = HudPainter::new(&mut canvas, &settings, Theme::street());
        painter.clear();
        // 先画测试图案（坐标轴与角度参考线），再画 HUD 本体，
        // 这样一张图能同时核对「坐标约定」与「仪表外观」。
        painter.draw_test_pattern();
        hud.paint(&mut painter, &trail_pts);
        painter.scanlines(
            Rect::new(0.0, 0.0, cw as f32, ch as f32),
            Bgra::rgb(255, 255, 255),
            0.05,
            4,
        );
        painter.halftone(
            Rect::new(0.0, ch as f32 * 0.55, cw as f32, ch as f32 * 0.45),
            Bgra::rgb(255, 255, 255),
            0.10,
            14,
            1.5,
        );
    }

    let out = cfg
        .selftest_out
        .clone()
        .unwrap_or_else(|| "hud-selftest.bmp".to_string());
    write_bmp(std::path::Path::new(&out), &canvas)?;
    Ok(out)
}

/// 把画布写成 32 位 BMP。
///
/// # 为什么自己写而不是用 GDI 的保存接口
///
/// GDI 没有「保存为 BMP」的公开 API（要 `GdiPlus`，那需要额外的库）。
/// BMP 的文件头结构简单且固定，自己写更省事，也不引入依赖。
///
/// 注意 BMP 原生像素顺序是**自下而上**，而我们的画布是自上而下的，
/// 因此写文件时要按行倒序。
fn write_bmp(path: &std::path::Path, canvas: &Canvas) -> Result<(), String> {
    let (w, h) = canvas.size();
    let row_bytes = w as usize * 4;
    let image_size = row_bytes * h as usize;
    let file_size = 14 + 40 + image_size;

    let mut f = File::create(path).map_err(|e| format!("创建 {} 失败: {e}", path.display()))?;

    let mut header = Vec::with_capacity(54);
    // ---- BITMAPFILEHEADER（14 字节）----
    header.extend_from_slice(b"BM");
    header.extend_from_slice(&(file_size as u32).to_le_bytes());
    header.extend_from_slice(&0u16.to_le_bytes()); // 保留
    header.extend_from_slice(&0u16.to_le_bytes()); // 保留
    header.extend_from_slice(&54u32.to_le_bytes()); // 像素数据偏移
                                                    // ---- BITMAPINFOHEADER（40 字节）----
    header.extend_from_slice(&40u32.to_le_bytes());
    header.extend_from_slice(&w.to_le_bytes());
    header.extend_from_slice(&h.to_le_bytes()); // 正高度 = 自下而上
    header.extend_from_slice(&1u16.to_le_bytes()); // 位平面
    header.extend_from_slice(&32u16.to_le_bytes()); // 32 位
    header.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    header.extend_from_slice(&(image_size as u32).to_le_bytes());
    header.extend_from_slice(&2835i32.to_le_bytes()); // 72 DPI
    header.extend_from_slice(&2835i32.to_le_bytes());
    header.extend_from_slice(&0u32.to_le_bytes());
    header.extend_from_slice(&0u32.to_le_bytes());

    f.write_all(&header)
        .map_err(|e| format!("写文件头失败: {e}"))?;

    // 逐行倒序写出
    let mut row = vec![0u8; row_bytes];
    for y in (0..h).rev() {
        for x in 0..w {
            let px = canvas
                .pixel(x, y)
                .ok_or_else(|| format!("读取像素 ({x},{y}) 越界"))?;
            let o = x as usize * 4;
            row[o] = px.b;
            row[o + 1] = px.g;
            row[o + 2] = px.r;
            row[o + 3] = px.a;
        }
        f.write_all(&row)
            .map_err(|e| format!("写像素行失败: {e}"))?;
    }
    f.flush().map_err(|e| format!("刷新失败: {e}"))?;
    Ok(())
}

/// 画诊断标记：四角红方块 + 画面正中十字 + 主仪表圆心处绿方块。
///
/// 用途：截图后核对标记是否正好落在画面四角。
/// 若标记不在角上（或间距不等于画布尺寸），说明窗口没有 1:1 铺满屏幕，
/// 此时问题在窗口/合成环节而不是绘制逻辑。
fn draw_diag_markers(cv: &mut Canvas, settings: &Settings) {
    let (cw, ch) = cv.size();
    let (cw, ch) = (cw as f32, ch as f32);
    let mark = Bgra::rgb(255, 0, 64);
    let s = 24.0f32;
    cv.fill_rect(Rect::new(0.0, 0.0, s, s), mark);
    cv.fill_rect(Rect::new(cw - s, 0.0, s, s), mark);
    cv.fill_rect(Rect::new(0.0, ch - s, s, s), mark);
    cv.fill_rect(Rect::new(cw - s, ch - s, s, s), mark);
    let cx = cw * 0.5;
    let cy = ch * 0.5;
    cv.fill_rect(Rect::new(cx - 60.0, cy - 2.0, 120.0, 4.0), mark);
    cv.fill_rect(Rect::new(cx - 2.0, cy - 60.0, 4.0, 120.0), mark);
    cv.fill_rect(
        Rect::new(settings.angle.cx - 8.0, settings.angle.cy - 8.0, 16.0, 16.0),
        Bgra::rgb(0, 255, 128),
    );
}

/// 抓帧模式：接收若干帧遥测，把**真实绘制的那一帧**存成 BMP，然后退出。
///
/// # 与 `--selftest` 的区别
///
/// | | `--selftest` | `--capture` |
/// |---|---|---|
/// | 数据来源 | 内置演示数据 | **实际收到的遥测**（无数据时即默认零值） |
/// | 绘制路径 | 与实时模式相同的 `HudFrame::paint` | 同上，另含 G 轨迹与判定器稳态 |
/// | 需要开窗口 | 否 | **否** |
///
/// 两者都不创建窗口。`--capture` 的用途是：当「屏幕上看到的」与「预期画的」
/// 不一致时，用它拿到程序**真正产出的那份像素**，从而把问题在
/// 「绘制逻辑」与「窗口合成」之间切开。
///
/// 它会监听 `--listen` 指定的端口最多 3 秒；没收到包就用默认零值照常抓帧
/// （此时画面会显示 NO DATA，与实时模式行为一致）。
fn run_capture(cfg: &Config) -> Result<String, String> {
    use std::net::UdpSocket;

    let app = load_app_config(cfg);
    let settings = app.hud.clone();
    let (cw, ch) = cfg.size.unwrap_or((1920, 1080));
    let mut canvas = Canvas::new(cw, ch)?;
    canvas.clear_transparent();

    let endpoint = listen_endpoint(cfg, &app);
    println!("监听 {endpoint} 最多 3 秒以获取真实遥测（没有数据也能照常抓帧）...");
    let mut latest: Option<Telemetry> = None;
    let mut packet_count = 0u64;
    match UdpSocket::bind(&endpoint) {
        Ok(sock) => {
            let _ = sock.set_read_timeout(Some(Duration::from_millis(300)));
            let start = Instant::now();
            let mut buf = [0u8; RECV_BUF];
            let mut last_recv: Option<Instant> = None;
            loop {
                if start.elapsed() > Duration::from_secs(3) {
                    break;
                }
                // 收到过包后，再等 0.5 秒没有新包就收尾
                if let Some(t) = last_recv {
                    if t.elapsed() > Duration::from_millis(500) {
                        break;
                    }
                }
                if let Ok((n, _)) = sock.recv_from(&mut buf) {
                    if let Ok(t) = Telemetry::parse(&buf[..n]) {
                        packet_count += 1;
                        latest = Some(t);
                        last_recv = Some(Instant::now());
                    }
                }
            }
        }
        Err(e) => {
            println!("  [!] 无法绑定 {endpoint}（{e}），将用默认零值抓帧。");
            println!("      若 HUD 正在运行，这是正常的 —— 端口被它占着。");
        }
    }

    let telemetry = latest.unwrap_or_default();
    if packet_count > 0 {
        println!("  收到 {packet_count} 个有效包，用最后一帧绘制。");
    } else {
        println!("  未收到有效包，用默认零值绘制（画面会显示 NO DATA / 全 0）。");
    }

    // 用真实遥测把判定器推进到稳态，使平滑值看起来与实际一致
    let mut judge = DriftJudge::new();
    let d = derived::derive(&telemetry);
    let mut drift = DriftSnapshot::default();
    for _ in 0..120 {
        drift = judge.update(&telemetry, &d, 1.0 / 60.0);
    }

    // G 轨迹：用当前 G 值造一小段，让轨迹绘制也出现在抓帧里
    let mut trail: TrailBuf<{ gauge::G_TRAIL_LEN }> = TrailBuf::new();
    for i in 0..gauge::G_TRAIL_LEN {
        let t = i as f32 / gauge::G_TRAIL_LEN as f32;
        trail.push((
            drift.g_lateral * (0.4 + 0.6 * t),
            drift.g_longitudinal * (0.4 + 0.6 * t),
        ));
    }
    let trail_pts: Vec<(f32, f32)> = trail.iter().collect();

    {
        let mut painter = HudPainter::new(&mut canvas, &settings, Theme::street());
        painter.clear();
        let hud = HudFrame::new(Frame::new(telemetry), drift, LinkState::NoData, 60.0, 0);
        hud.paint(&mut painter, &trail_pts);
    }

    let out = cfg
        .capture_out
        .clone()
        .unwrap_or_else(|| "hud-capture.bmp".to_string());
    write_bmp(std::path::Path::new(&out), &canvas)?;
    println!("  抓帧尺寸: {cw}x{ch}");
    Ok(out)
}

/// 实时模式。
fn run_live(cfg: &Config) -> Result<(), String> {
    let app = load_app_config(cfg);
    let settings = app.hud.clone();
    let endpoint = listen_endpoint(cfg, &app);

    // ---- 0. 初始化日志（配置里没开启就什么也不做）----
    app.init_logging("fh6-hud");
    let log = fh6_config::Logger::global();
    log.info(&format!(
        "外观: 语言={} 面板透明度={:.2} G表底衬={:.2} 弹窗底衬={:.2}",
        settings.language.as_str(),
        settings.input_panel.panel_opacity,
        settings.g_meter.backdrop_alpha,
        settings.popup.backdrop_alpha
    ));

    // ---- 1. 启动收包线程 ----
    let slot = Arc::new(TelemetrySlot::new());
    let stop = Arc::new(StopFlag::new());
    let receiver = spawn_receiver(endpoint.clone(), Arc::clone(&slot), Arc::clone(&stop))?;

    // ---- 2. 创建窗口 ----
    let opts = match cfg.size {
        Some((w, h)) => WindowOptions {
            x: 0,
            y: 0,
            width: w,
            height: h,
        },
        None => WindowOptions::full_virtual_screen(),
    };
    let mut win = LayeredWindow::create(opts)?;

    println!("fh6-hud {} —— FH6 HUD", env!("CARGO_PKG_VERSION"));
    println!("遥测来源: {endpoint}");
    println!("界面语言: {}", settings.language.display_name());
    println!(
        "窗口尺寸: {}x{} @ ({}, {})",
        opts.width, opts.height, opts.x, opts.y
    );
    log.info(&format!(
        "窗口 {}x{} @ ({},{})，监听 {endpoint}",
        opts.width, opts.height, opts.x, opts.y
    ));
    if cfg.diag {
        // 诊断模式：把「我们以为的尺寸」全部打出来，并在四角画标记。
        // 用途：如果截图上标记不在画面四角，说明窗口没有 1:1 铺满屏幕。
        let (cw, ch) = win.canvas().size();
        println!("==== 诊断 ====");
        println!("  画布尺寸 (Canvas)      : {cw} x {ch}");
        println!(
            "  窗口请求尺寸 (opts)    : {} x {} @ ({}, {})",
            opts.width, opts.height, opts.x, opts.y
        );
        println!("  是否请求置顶           : {}", !cfg.no_topmost);
        println!(
            "  脏区重绘启用           : {}",
            settings.perf.dirty_region_redraw
        );
        println!(
            "  活动/静止帧率上限      : {} / {} fps",
            settings.perf.active_fps, settings.perf.idle_fps
        );
        println!(
            "  设计基准               : 主仪表圆心 ({}, {}) 半径 {}",
            settings.angle.cx, settings.angle.cy, settings.angle.radius
        );
        println!("  预期标记位置           : 四角各一个 24x24 方块，中心另有一个十字");

        println!("  已上传标记帧。请截图，并核对四个红方块是否正好在画面四角。");
        println!("  按 Ctrl+C 退出。");
    }

    if cfg.no_topmost {
        println!("注意: 已请求不置顶，窗口可能被游戏盖住（仅用于调试）。");
    }
    println!("按 Esc（HUD 拿到焦点时）或 Ctrl+C 退出。");
    println!();

    // ---- 3. 渲染主循环 ----
    let result = render_loop(cfg.diag, &mut win, &slot, &stop, &settings);

    // ---- 4. 收尾 ----
    stop.request();
    let _ = receiver.join();

    if let Some(path) = &cfg.save_config {
        // 保存的是**完整** AppConfig（含 relay 与日志段），
        // 这样 HUD 的保存动作不会把 relay 的配置抹掉。
        match app.save(std::path::Path::new(path)) {
            Ok(_) => println!("已保存完整配置到 {path}"),
            Err(e) => eprintln!("保存配置失败: {e}"),
        }
    }

    result
}

/// 启动收包线程。
fn spawn_receiver(
    listen: String,
    slot: Arc<TelemetrySlot>,
    stop: Arc<StopFlag>,
) -> Result<thread::JoinHandle<()>, String> {
    // 在**主线程**绑定：绑定失败要立刻报错，不要在线程里静默失败
    let socket = UdpSocket::bind(&listen).map_err(|e| {
        format!(
            "无法绑定 {listen}: {e}\n\
             提示：端口可能已被占用。可用 
etstat -ano -p UDP | findstr :{}` 查看。",
            listen.rsplit(':').next().unwrap_or("5302")
        )
    })?;
    socket
        .set_read_timeout(Some(Duration::from_millis(100)))
        .map_err(|e| format!("设置读取超时失败: {e}"))?;

    let handle = thread::spawn(move || {
        let mut buf = [0u8; RECV_BUF];
        // 用「阻塞 + 超时」而不是非阻塞轮询：无数据时线程在系统调用里睡眠，
        // CPU 占用为 0；超时只用于定期检查停止标志。
        while !stop.is_requested() {
            match socket.recv_from(&mut buf) {
                Ok((len, _from)) => match Telemetry::parse(&buf[..len]) {
                    Ok(t) => {
                        let frame = Frame::new(t);
                        slot.publish(frame, t.timestamp_ms, Instant::now());
                    }
                    Err(_) => slot.note_rejected(),
                },
                Err(e) => {
                    // 超时与 ICMP 端口不可达都是正常现象，继续循环。
                    // 若在这里 break，游戏一发第一个包或者一次超时就会让线程退出。
                    if e.kind() != std::io::ErrorKind::WouldBlock
                        && e.kind() != std::io::ErrorKind::TimedOut
                    {
                        thread::sleep(Duration::from_millis(1));
                    }
                }
            }
        }
    });

    Ok(handle)
}

/// 渲染主循环。
fn render_loop(
    diag: bool,
    win: &mut LayeredWindow,
    slot: &Arc<TelemetrySlot>,
    stop: &Arc<StopFlag>,
    settings: &Settings,
) -> Result<(), String> {
    let theme = Theme::street();
    let mut judge = DriftJudge::new();
    let mut clock = FrameClock::new(
        settings.perf.active_fps,
        settings.perf.idle_fps,
        settings.perf.idle_throttle,
    );
    let mut trail: TrailBuf<{ gauge::G_TRAIL_LEN }> = TrailBuf::new();
    let mut trail_pts: Vec<(f32, f32)> = Vec::with_capacity(gauge::G_TRAIL_LEN);
    let mut dirty = DirtyRegion::new();
    let mut fps_avg = MovingAverage::new(30);
    let mut last_frame_time = Instant::now();
    let mut last_fps_sample = Instant::now();
    let mut frames_since_sample = 0u32;
    let mut prev_speed = f32::NAN;
    let mut prev_drift = DriftSnapshot::default();
    let mut prev_link = LinkState::NoData;
    let mut stat_timer = Instant::now();
    let mut stat_frames = 0u64;
    let mut stat_pixels = 0u64;

    let canvas_size = win.canvas().size();

    loop {
        // ---- 消息泵 ----
        win.pump_messages();
        for key in win.take_keys() {
            if key.vk == fh6_hud::win32::vk::ESCAPE {
                stop.request();
            }
        }
        let _ = win.mouse_state();
        let _ = win.take_wheel();
        let user_wants_render = win.take_render_request();

        if win.should_quit() || stop.is_requested() {
            break;
        }

        // ---- 读取最新遥测 ----
        let now = Instant::now();
        let (core_frame, link, _age) = slot.read(now);

        let dt = now.saturating_duration_since(last_frame_time).as_secs_f32();
        last_frame_time = now;

        let changed = clock.observe(&core_frame);
        let d = derived::derive(&core_frame.telemetry);
        let drift = judge.update(&core_frame.telemetry, &d, dt);

        // ---- 帧率节流 ----
        if !clock.should_render(now) {
            let wait = clock.time_until_next(now).min(Duration::from_millis(8));
            if !wait.is_zero() {
                thread::sleep(wait);
            }
            continue;
        }

        // ---- 帧率统计 ----
        frames_since_sample += 1;
        let sample_elapsed = now.saturating_duration_since(last_fps_sample);
        if sample_elapsed >= Duration::from_millis(250) {
            fps_avg.push(frames_since_sample as f32 / sample_elapsed.as_secs_f32().max(0.001));
            frames_since_sample = 0;
            last_fps_sample = now;
        }
        let fps = fps_avg.average().unwrap_or(0.0);

        // ---- 计算脏区 ----
        //
        // 这是本项目相对参考实现最关键的资源优化：
        // 只有「数值确实变了」或「用户操作了」才登记脏区；
        // 都没有时脏区为空 → 直接跳过上传，**一帧都不画**。
        dirty.clear();
        let full = Rect::new(0.0, 0.0, canvas_size.0 as f32, canvas_size.1 as f32);

        let speed_changed = !approx_eq(prev_speed, d.speed_kph);
        let drift_changed = drift_is_different(&prev_drift, &drift);
        let link_changed = link != prev_link;
        // 弹窗在消失的那一帧也必须重绘一次，否则会残留在屏幕上
        let popup_active = drift.popup.popup.is_some() || prev_drift.popup.popup.is_some();

        if diag {
            // 诊断模式下强制每帧上传：标记需要持续可见，
            // 否则静止时脏区为空、标记根本不会上屏。
            dirty.mark(full);
        } else if !settings.perf.dirty_region_redraw {
            // 关掉脏区重绘时退化为「每帧全屏重绘」——即参考实现的行为。
            // 保留这个开关是为了能实测对比两种策略的开销。
            dirty.mark(full);
        } else if changed
            || speed_changed
            || drift_changed
            || link_changed
            || popup_active
            || user_wants_render
        {
            // 面板分布在四角，包围盒本来就接近整屏，因此这里直接标整屏；
            // 真正省下开销的是「静止时完全不上传」这条路径。
            dirty.mark(full);
        }

        // ---- 绘制并上传 ----
        if let Some(region) = dirty.bbox() {
            // 先把 uploads 读出来：canvas_mut() 需要一个可变借用，
            // 若在同一表达式里再调用 &self 方法会冲突。
            let uploads_before = win.uploads();
            {
                let mut painter = HudPainter::new(win.canvas_mut(), settings, theme);
                painter.clear();
                if changed {
                    trail.push((drift.g_lateral, drift.g_longitudinal));
                }
                trail_pts.clear();
                trail_pts.extend(trail.iter());
                let hud = HudFrame::new(core_frame, drift, link, fps, uploads_before);
                hud.paint(&mut painter, &trail_pts);
            }
            if diag {
                draw_diag_markers(win.canvas_mut(), settings);
            }
            let uploaded = win.upload(Some(region))?;
            stat_pixels += uploaded;
            stat_frames += 1;
        }

        prev_speed = d.speed_kph;
        prev_drift = drift;
        prev_link = link;

        // ---- 每 5 秒打印一次统计（便于核对脏区重绘是否真的生效）----
        if stat_timer.elapsed() >= Duration::from_secs(5) {
            println!(
                "统计: {:.0} fps | 上传 {} 次 / {:.1} M 像素 | 收包 {} (解析 {} 拒绝 {}) | 链接 {}",
                fps,
                win.uploads(),
                win.uploaded_pixels() as f64 / 1_000_000.0,
                slot.packets_received(),
                slot.packets_parsed(),
                slot.packets_rejected(),
                link.label()
            );
            let _ = (stat_frames, stat_pixels);
            stat_frames = 0;
            stat_pixels = 0;
            stat_timer = Instant::now();
        }
    }

    Ok(())
}

/// 近似相等，任一非有限值视为「不相等」（需要重绘）。
fn approx_eq(a: f32, b: f32) -> bool {
    if !a.is_finite() || !b.is_finite() {
        return false;
    }
    (a - b).abs() <= 0.01
}

/// 判断两个漂移快照是否需要重绘。
///
/// 只比较**会被画出来**的量，避免内部状态（如恢复帧计数）造成无意义重绘。
fn drift_is_different(a: &DriftSnapshot, b: &DriftSnapshot) -> bool {
    if a.drift_state != b.drift_state
        || a.style != b.style
        || a.risk_label != b.risk_label
        || a.flow_quality != b.flow_quality
        || a.popup.popup != b.popup.popup
        || a.popup.operation != b.popup.operation
        || a.holding_drift != b.holding_drift
        || a.countering != b.countering
        || a.god_word != b.god_word
    {
        return true;
    }
    let pairs: [(f32, f32); 9] = [
        (a.display_angle, b.display_angle),
        (a.g_lateral, b.g_lateral),
        (a.g_longitudinal, b.g_longitudinal),
        (a.counter_pct, b.counter_pct),
        (a.flow_pct, b.flow_pct),
        (a.spin_risk, b.spin_risk),
        (a.hold_seconds, b.hold_seconds),
        (a.popup.ttl, b.popup.ttl),
        (a.popup.operation_ttl, b.popup.operation_ttl),
    ];
    pairs.iter().any(|(x, y)| !approx_eq(*x, *y))
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    #[test]
    fn 默认监听5302端口() {
        // 命令行未指定 --listen 时，端点取自配置里的 hud.network。
        // 默认值与 fh6-relay 的默认扇出目标一致：
        // 游戏发 5301 → relay → 5300（飞智）+ 5302（本 HUD）。
        assert_eq!(Config::default().listen, None);
        let app = AppConfig::default();
        assert_eq!(app.hud.network.listen_port, 5302);
        assert_eq!(app.hud.network.endpoint(), "127.0.0.1:5302");
    }

    #[test]
    fn 监听地址拆分支持多种写法() {
        assert_eq!(
            split_host_port("127.0.0.1:5302"),
            Some(("127.0.0.1".to_string(), 5302))
        );
        // 只给端口：地址回退到回环
        assert_eq!(
            split_host_port(":5302"),
            Some(("127.0.0.1".to_string(), 5302))
        );
        // 纯端口号也接受
        assert_eq!(
            split_host_port("5400"),
            Some(("127.0.0.1".to_string(), 5400))
        );
        // 允许监听全部接口
        assert_eq!(
            split_host_port("0.0.0.0:5302"),
            Some(("0.0.0.0".to_string(), 5302))
        );
        // 前后空白被忽略
        assert_eq!(
            split_host_port("  127.0.0.1:5301  "),
            Some(("127.0.0.1".to_string(), 5301))
        );
        // 非法输入
        assert_eq!(split_host_port("127.0.0.1:abc"), None);
        assert_eq!(
            split_host_port("127.0.0.1:99999"),
            None,
            "端口超出 u16 范围"
        );
        assert_eq!(split_host_port(""), None);
    }

    #[test]
    fn 参数解析默认进入实时模式() {
        match parse_args(&[]).unwrap() {
            Action::Run(c) => {
                assert_eq!(c.listen, None, "未指定 --listen 时应为 None（端点取配置）");
                assert!(c.selftest_out.is_none());
            }
            _ => panic!("默认应为实时模式"),
        }
    }

    #[test]
    fn selftest参数进入自检模式并带默认输出名() {
        match parse_args(&["--selftest".to_string()]).unwrap() {
            Action::SelfTest(c) => {
                assert_eq!(c.selftest_out.as_deref(), Some("hud-selftest.bmp"))
            }
            _ => panic!("应进入自检模式"),
        }
    }

    #[test]
    fn selftest可指定输出路径() {
        match parse_args(&["--selftest".to_string(), "out.bmp".to_string()]).unwrap() {
            Action::SelfTest(c) => assert_eq!(c.selftest_out.as_deref(), Some("out.bmp")),
            _ => panic!("应进入自检模式"),
        }
    }

    #[test]
    fn help与version() {
        assert!(matches!(
            parse_args(&["--help".to_string()]).unwrap(),
            Action::Help
        ));
        assert!(matches!(
            parse_args(&["-V".to_string()]).unwrap(),
            Action::Version
        ));
    }

    #[test]
    fn 等号写法被支持() {
        match parse_args(&[
            "--listen=127.0.0.1:5400".to_string(),
            "--size=1280x720".to_string(),
        ])
        .unwrap()
        {
            Action::Run(c) => {
                assert_eq!(c.listen.as_deref(), Some("127.0.0.1:5400"));
                assert_eq!(c.size, Some((1280, 720)));
            }
            _ => panic!("应为实时模式"),
        }
    }

    #[test]
    fn 尺寸解析支持两种分隔符() {
        assert_eq!(parse_size("1920x1080").unwrap(), (1920, 1080));
        assert_eq!(parse_size("1920*1080").unwrap(), (1920, 1080));
        assert_eq!(parse_size(" 800 X 600 ").unwrap(), (800, 600));
    }

    #[test]
    fn 尺寸解析拒绝非法输入() {
        assert!(parse_size("1920").is_err());
        assert!(parse_size("axb").is_err());
        assert!(parse_size("10x10").is_err(), "过小应被拒绝");
        assert!(parse_size("1920x0").is_err());
    }

    #[test]
    fn 缺少参数的选项报错() {
        assert!(parse_args(&["-l".to_string()]).is_err());
        assert!(parse_args(&["--config".to_string()]).is_err());
        assert!(parse_args(&["--size".to_string()]).is_err());
        assert!(parse_args(&["--unknown".to_string()]).is_err());
    }

    #[test]
    fn 漂移快照差异检测覆盖关键字段() {
        let a = DriftSnapshot::default();
        let mut b = a;
        b.flow_pct = 50.0;
        assert!(drift_is_different(&a, &b), "FLOW 变化应被检测到");

        let mut c = a;
        c.display_angle = 20.0;
        assert!(drift_is_different(&a, &c), "角度变化应被检测到");

        let mut e = a;
        e.risk_label = fh6_core::drift::RiskLabel::Max;
        assert!(drift_is_different(&a, &e), "风险标签变化应被检测到");

        // 完全相同不应被判为变化（否则静止时仍会满帧重绘）
        assert!(!drift_is_different(&a, &a));
    }

    #[test]
    fn 微小数值抖动不触发重绘() {
        let a = DriftSnapshot::default();
        let mut b = a;
        b.flow_pct = 0.005; // 小于 0.01 容差
        assert!(!drift_is_different(&a, &b), "小于容差的变化不应触发重绘");
    }

    #[test]
    fn 近似相等对非有限值的处理() {
        assert!(approx_eq(1.0, 1.0));
        assert!(approx_eq(1.0, 1.005));
        assert!(!approx_eq(1.0, 1.05));
        assert!(!approx_eq(f32::NAN, f32::NAN));
        assert!(!approx_eq(1.0, f32::INFINITY));
    }

    #[test]
    fn 演示数据能产生有内容的快照() {
        // 自检图不能是一片空白：演示数据必须能把判定器推进到有意义的状态
        let (core, drift) = demo_hud_frame();
        assert!(core.derived.speed_kph > 100.0, "演示速度应约 120 km/h");
        assert_eq!(core.derived.gear_raw, 4);
        assert!(
            drift.display_angle.abs() > 10.0,
            "演示数据应产生明显漂移角, 实际 {}",
            drift.display_angle
        );
        assert!(drift.flow_pct > 0.0, "FLOW 应有读数");
    }

    #[test]
    fn bmp写入产生合法文件头() {
        let canvas = Canvas::new(8, 4).expect("创建画布失败");
        // 注意：不要写到 std::env::temp_dir() —— 在中文 Windows 上该路径含非 ASCII
        // 字符，而 std::fs::File::create 在 Windows 上走的是非 Unicode 路径 API，
        // 会直接失败。写到当前工作目录（构建目录）最稳。
        let path = std::env::current_dir()
            .expect("取当前目录失败")
            .join("fh6_hud_bmp_test.bmp");
        write_bmp(&path, &canvas).expect("写 BMP 失败");

        let data = std::fs::read(&path).expect("读回失败");
        assert_eq!(data.len(), 54 + 8 * 4 * 4, "文件大小 = 头 + 像素");
        assert_eq!(&data[0..2], b"BM", "BMP magic");
        let declared = u32::from_le_bytes([data[2], data[3], data[4], data[5]]);
        assert_eq!(
            declared as usize,
            data.len(),
            "文件头声明的大小应等于实际大小"
        );
        let w = i32::from_le_bytes([data[18], data[19], data[20], data[21]]);
        let h = i32::from_le_bytes([data[22], data[23], data[24], data[25]]);
        assert_eq!(w, 8);
        assert_eq!(h, 4, "正高度表示自下而上存储");
        let bpp = u16::from_le_bytes([data[28], data[29]]);
        assert_eq!(bpp, 32);

        let _ = std::fs::remove_file(&path);
    }
}
