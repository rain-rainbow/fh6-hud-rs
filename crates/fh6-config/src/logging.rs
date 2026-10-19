//! 可选的文件日志。
//!
//! # 为什么手写而不引入 `log` + `tracing`
//!
//! 本项目只需要「按行写文件 + 大小轮转 + 四个级别」，`log` crate 的生态
//! （`env_logger`/`tracing-subscriber`）会带进十来个传递依赖，而产物体积
//! 是这个项目的核心指标之一（release exe 目前约 500 KB）。手写约 250 行，
//! 且**每一行都能单元测试**，可控性更高。
//!
//! # 轮转策略
//!
//! 写入前检查「当前文件大小 + 本行长度」是否超过 `max_bytes`；超过则：
//!
//! ```text
//! app.log      -> app.log.1
//! app.log.1    -> app.log.2
//! ...
//! app.log.(N-1) -> app.log.N   （超出的删除）
//! app.log      -> 新建
//! ```
//!
//! 这样磁盘占用上界约为 `max_bytes × (keep_files + 1)`，可预测。
//!
//! # 失败处理
//!
//! 日志**永远不能影响主功能**：文件打不开就退化为只输出控制台；
//! 写失败就静默丢弃该行。因此所有方法都不返回错误。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// 日志级别，数值越大越啰嗦。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// 只记错误。
    Error,
    /// 错误 + 警告。
    Warn,
    /// 错误 + 警告 + 关键事件（默认）。
    #[default]
    Info,
    /// 全部，含每个遥测包的调试信息。
    Debug,
}

impl LogLevel {
    /// 从字符串解析，无法识别时回退 `Info`。
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "error" | "err" => LogLevel::Error,
            "warn" | "warning" => LogLevel::Warn,
            "debug" | "trace" => LogLevel::Debug,
            _ => LogLevel::Info,
        }
    }

    /// 配置里的字符串形式。
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
        }
    }

    /// 写进日志的固定宽度标签。
    pub fn tag(self) -> &'static str {
        match self {
            LogLevel::Error => "ERROR",
            LogLevel::Warn => "WARN ",
            LogLevel::Info => "INFO ",
            LogLevel::Debug => "DEBUG",
        }
    }
}

/// 日志配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogConfig {
    /// 是否写文件。关闭时完全不做文件 I/O。
    pub enabled: bool,
    /// 日志文件路径。相对路径按**进程工作目录**解析。
    pub path: String,
    /// 级别。
    pub level: LogLevel,
    /// 单文件大小上限（字节），超过即轮转。
    pub max_bytes: u64,
    /// 保留的历史份数。
    pub keep_files: u32,
    /// 是否同时输出到控制台（stdout）。
    pub console: bool,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            path: "logs/fh6-hud.log".to_string(),
            level: LogLevel::Info,
            // 8 MB × (3+1) = 32 MB 上界，对日志来说足够且不会失控
            max_bytes: 8 * 1024 * 1024,
            keep_files: 3,
            console: true,
        }
    }
}

impl LogConfig {
    /// 校验并规范化。
    ///
    /// 返回 `true` 表示发生了修正。
    pub fn sanitize(&mut self) -> bool {
        let before = self.clone();
        let p = self.path.trim();
        if p.is_empty() || p.len() > 512 {
            self.path = "logs/fh6-hud.log".to_string();
        } else {
            self.path = p.to_string();
        }
        // 下限 4 KB、上限 1 GB：给小了会疯狂轮转，给大了可能撑爆磁盘
        self.max_bytes = self.max_bytes.clamp(4 * 1024, 1024 * 1024 * 1024);
        self.keep_files = self.keep_files.min(50);
        *self != before
    }

    /// 日志文件所在目录（不存在则返回当前目录）。
    pub fn dir(&self) -> PathBuf {
        Path::new(&self.path)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

/// 全局日志器。
pub struct Logger {
    cfg: LogConfig,
    /// 当前文件与其已写入字节数。
    state: Mutex<Option<(File, u64)>>,
}

static GLOBAL: OnceLock<Logger> = OnceLock::new();

impl Logger {
    /// 初始化全局日志器（幂等：重复调用返回已有实例）。
    ///
    /// 文件打不开时自动退化为仅控制台，不报错、不阻塞启动。
    pub fn init(cfg: &LogConfig) -> &'static Logger {
        GLOBAL.get_or_init(|| Logger::new(cfg))
    }

    /// 取全局日志器；未初始化时返回一个禁用文件的实例。
    ///
    /// 这样调用点不必关心初始化顺序。
    pub fn global() -> &'static Logger {
        GLOBAL.get_or_init(|| Logger::new(&LogConfig::default()))
    }

    /// 新建（一般用 [`Logger::init`]）。
    pub fn new(cfg: &LogConfig) -> Self {
        let state = if cfg.enabled {
            open_log_file(cfg)
        } else {
            None
        };
        Self {
            cfg: cfg.clone(),
            state: Mutex::new(state),
        }
    }

    /// 当前配置。
    pub fn config(&self) -> &LogConfig {
        &self.cfg
    }

    /// 该级别是否会被记录。
    pub fn enabled(&self, level: LogLevel) -> bool {
        level <= self.cfg.level
    }

    /// 写一行日志。
    pub fn log(&self, level: LogLevel, msg: &str) {
        if !self.enabled(level) {
            return;
        }
        let line = format!("{} {} {}\n", utc_timestamp_now(), level.tag(), msg);
        if self.cfg.console {
            // 直接写 stdout，避免 println! 的格式化开销差异
            print!("{line}");
            let _ = std::io::stdout().flush();
        }
        if !self.cfg.enabled {
            return;
        }
        let Ok(mut guard) = self.state.lock() else {
            return;
        };
        let Some((file, written)) = guard.as_mut() else {
            return;
        };
        // 轮转检查
        if *written + line.len() as u64 > self.cfg.max_bytes {
            drop(guard);
            let new_state = rotate(&self.cfg);
            if let Ok(mut g) = self.state.lock() {
                *g = new_state;
                if let Some((f, w)) = g.as_mut() {
                    let _ = f.write_all(line.as_bytes());
                    let _ = f.flush();
                    *w = line.len() as u64;
                }
            }
            return;
        }
        if file.write_all(line.as_bytes()).is_ok() {
            *written += line.len() as u64;
            let _ = file.flush();
        }
    }

    /// 错误级。
    pub fn error(&self, msg: &str) {
        self.log(LogLevel::Error, msg);
    }
    /// 警告级。
    pub fn warn(&self, msg: &str) {
        self.log(LogLevel::Warn, msg);
    }
    /// 信息级。
    pub fn info(&self, msg: &str) {
        self.log(LogLevel::Info, msg);
    }
    /// 调试级。
    pub fn debug(&self, msg: &str) {
        self.log(LogLevel::Debug, msg);
    }
}

/// 打开（并准备追加）日志文件，同时报告当前大小。
fn open_log_file(cfg: &LogConfig) -> Option<(File, u64)> {
    let path = Path::new(&cfg.path);
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            let _ = fs::create_dir_all(dir);
        }
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()?;
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    Some((file, size))
}

/// 轮转：`x.log` → `x.log.1` → …，超过 `keep_files` 的丢弃。
fn rotate(cfg: &LogConfig) -> Option<(File, u64)> {
    let path = Path::new(&cfg.path);
    // 先把最旧的删掉
    if cfg.keep_files > 0 {
        for i in (1..=cfg.keep_files).rev() {
            let src = path_with_suffix(path, i);
            let dst = path_with_suffix(path, i + 1);
            if i == cfg.keep_files {
                let _ = fs::remove_file(&src);
            } else if src.exists() {
                let _ = fs::rename(&src, &dst);
            }
        }
        let first = path_with_suffix(path, 1);
        let _ = fs::rename(path, &first);
    } else {
        let _ = fs::remove_file(path);
    }
    open_log_file(cfg)
}

/// 给路径追加 `.N` 后缀。
fn path_with_suffix(path: &Path, n: u32) -> PathBuf {
    let mut p = path.as_os_str().to_os_string();
    p.push(format!(".{n}"));
    PathBuf::from(p)
}

/// 当前 UTC 时间戳，格式 `YYYY-MM-DD HH:MM:SS`。
fn utc_timestamp_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_utc(secs)
}

/// 把 Unix 秒格式化为 `YYYY-MM-DD HH:MM:SS`（UTC）。
///
/// 独立的函数便于测试；日期换算用 Howard Hinnant 的 civil_from_days 算法，
/// 对 1970-2100 范围内的任意日期都正确。
pub fn format_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02}")
}

/// 自 1970-01-01 起的天数 → (年, 月, 日)。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    #[test]
    fn 默认配置合法() {
        let mut c = LogConfig::default();
        assert!(!c.sanitize(), "默认值不应需要修正");
        assert!(!c.enabled, "默认不写文件");
        assert_eq!(c.level, LogLevel::Info);
    }

    #[test]
    fn 级别解析与字符串往返() {
        assert_eq!(LogLevel::parse("error"), LogLevel::Error);
        assert_eq!(LogLevel::parse("WARN"), LogLevel::Warn);
        assert_eq!(LogLevel::parse("Debug"), LogLevel::Debug);
        assert_eq!(
            LogLevel::parse("乱七八糟"),
            LogLevel::Info,
            "无法识别应回退 Info"
        );
        for l in [
            LogLevel::Error,
            LogLevel::Warn,
            LogLevel::Info,
            LogLevel::Debug,
        ] {
            assert_eq!(LogLevel::parse(l.as_str()), l);
        }
    }

    #[test]
    fn 级别过滤正确() {
        let cfg = LogConfig {
            level: LogLevel::Warn,
            ..Default::default()
        };
        let logger = Logger::new(&cfg);
        assert!(logger.enabled(LogLevel::Error));
        assert!(logger.enabled(LogLevel::Warn));
        assert!(!logger.enabled(LogLevel::Info));
        assert!(!logger.enabled(LogLevel::Debug));
    }

    #[test]
    fn 时间戳格式正确() {
        // 1970-01-01 00:00:00 UTC
        assert_eq!(format_utc(0), "1970-01-01 00:00:00");
        // 2000-01-01 00:00:00 UTC = 946684800
        assert_eq!(format_utc(946_684_800), "2000-01-01 00:00:00");
        // 2024-02-29 12:34:56 UTC（闰日）= 1709210096
        assert_eq!(format_utc(1_709_210_096), "2024-02-29 12:34:56");
        // 2026-10-19 00:00:00 UTC = 1792368000
        assert_eq!(format_utc(1_792_368_000), "2026-10-19 00:00:00");
    }

    #[test]
    fn 闰年边界正确() {
        // 2024-03-01 00:00:00 UTC = 1709251200
        assert_eq!(format_utc(1_709_251_200), "2024-03-01 00:00:00");
        // 2100 不是闰年
        assert_eq!(format_utc(4_107_542_400), "2100-03-01 00:00:00");
    }

    #[test]
    fn 路径后缀拼接正确() {
        let p = Path::new("logs/app.log");
        assert_eq!(path_with_suffix(p, 1), PathBuf::from("logs/app.log.1"));
        assert_eq!(path_with_suffix(p, 12), PathBuf::from("logs/app.log.12"));
    }

    #[test]
    fn 日志目录取自路径父目录() {
        let c = LogConfig {
            path: "some/dir/a.log".to_string(),
            ..Default::default()
        };
        assert_eq!(c.dir(), PathBuf::from("some/dir"));
    }

    #[test]
    fn 实际写文件并按级别过滤() {
        let dir = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("logtest");
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("t.log");
        let mut cfg = LogConfig {
            enabled: true,
            path: path.to_string_lossy().to_string(),
            level: LogLevel::Warn,
            console: false,
            ..Default::default()
        };
        cfg.sanitize();
        let logger = Logger::new(&cfg);
        logger.info("这条不该出现");
        logger.warn("警告一");
        logger.error("错误一");
        let text = fs::read_to_string(&path).unwrap_or_default();
        assert!(!text.contains("这条不该出现"), "低于级别的日志不应写入");
        assert!(text.contains("WARN"), "应包含警告行");
        assert!(text.contains("ERROR"), "应包含错误行");
        assert!(text.contains("警告一"));
        assert!(text.contains("错误一"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 超过大小上限会轮转() {
        let dir = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("logrot");
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("r.log");
        let mut cfg = LogConfig {
            enabled: true,
            path: path.to_string_lossy().to_string(),
            level: LogLevel::Debug,
            console: false,
            // 最小值 4 KB；写够多行必然触发轮转
            max_bytes: 4096,
            keep_files: 2,
        };
        cfg.sanitize();
        let logger = Logger::new(&cfg);
        // 每行约 40 字节，写 400 行 ≈ 16 KB，必然轮转多次
        for i in 0..400 {
            logger.info(&format!("这是第 {i} 行，用于触发轮转的填充内容"));
        }
        assert!(path.exists(), "当前日志文件应存在");
        let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        assert!(size <= cfg.max_bytes, "当前文件不应超过上限, 实际 {size}");
        // 至少应有一个历史文件
        let has_hist = (1..=cfg.keep_files).any(|i| path_with_suffix(&path, i).exists());
        assert!(has_hist, "应生成轮转后的历史文件");
        // 保留份数不应超过 keep_files
        let hist_count = (1..=10u32)
            .filter(|i| path_with_suffix(&path, *i).exists())
            .count();
        assert!(
            hist_count <= cfg.keep_files as usize,
            "历史份数 {hist_count} 不应超过 {}",
            cfg.keep_files
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 关闭时完全不写文件() {
        let dir = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("logoff");
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("off.log");
        let cfg = LogConfig {
            enabled: false,
            path: path.to_string_lossy().to_string(),
            console: false,
            ..Default::default()
        };
        let logger = Logger::new(&cfg);
        logger.error("不应创建文件");
        assert!(!path.exists(), "关闭时不应创建任何文件");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 异常路径不影响运行() {
        // 指向一个非法路径：初始化应退化为无文件，但不 panic
        let cfg = LogConfig {
            enabled: true,
            path: "?:/非法<>路径/x.log".to_string(),
            console: false,
            ..Default::default()
        };
        let logger = Logger::new(&cfg);
        logger.info("不应 panic");
    }

    #[test]
    fn 大小与保留份数被夹紧() {
        let mut c = LogConfig {
            max_bytes: 1,
            keep_files: 9999,
            ..Default::default()
        };
        c.sanitize();
        assert_eq!(c.max_bytes, 4 * 1024);
        assert_eq!(c.keep_files, 50);

        let mut c2 = LogConfig {
            max_bytes: u64::MAX,
            ..Default::default()
        };
        c2.sanitize();
        assert_eq!(c2.max_bytes, 1024 * 1024 * 1024);
    }

    #[test]
    fn 空路径回退默认值() {
        let mut c = LogConfig {
            path: "  ".to_string(),
            ..Default::default()
        };
        c.sanitize();
        assert_eq!(c.path, "logs/fh6-hud.log");
    }

    #[test]
    fn json往返无损() {
        let c = LogConfig {
            enabled: true,
            path: "D:/logs/x.log".to_string(),
            level: LogLevel::Debug,
            max_bytes: 1234,
            keep_files: 5,
            console: false,
        };
        let json = serde_json::to_string(&c).unwrap();
        let back: LogConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
    }
}
