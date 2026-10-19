//! 顶层配置文件：把 HUD、relay、日志三部分装在一个文件里。
//!
//! # 为什么合并成一个文件
//!
//! 三个组件（HUD、relay、GUI）要读同一份配置。拆成三个文件时，用户改完
//! 一个忘了改另一个就是常见故障。合成一个 `AppConfig` 后：
//!
//! * GUI 只需要读写一处；
//! * 两个二进制共用同一个解析与校验路径；
//! * 版本号统一，迁移逻辑只有一份。
//!
//! # 原子保存
//!
//! 保存时先写 `xxx.json.tmp`，再 `rename` 覆盖正式文件。这样即使写到一半
//! 崩溃/断电，正式文件仍是上一次的完整内容，不会变成半截 JSON
//! （否则下次启动就会因解析失败回退默认值，用户的所有调参全丢）。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::apps::AppsConfig;
use crate::logging::LogConfig;
use crate::relay::RelayConfig;
use crate::settings::{Settings, SETTINGS_VERSION};

/// 当前顶层配置版本。
pub const APP_CONFIG_VERSION: u32 = 1;

/// 默认配置文件名（放在可执行文件同目录）。
pub const DEFAULT_FILE_NAME: &str = "fh6-hud.config.json";

/// 应用的完整配置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// 顶层版本号。
    pub version: u32,
    /// HUD 外观与接收端点。
    pub hud: Settings,
    /// relay 监听点与分发目标。
    pub relay: RelayConfig,
    /// 日志设置。
    pub logging: LogConfig,
    /// GUI 应用路径与启动行为。
    pub apps: AppsConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: APP_CONFIG_VERSION,
            hud: Settings::default(),
            relay: RelayConfig::default(),
            logging: LogConfig::default(),
            apps: AppsConfig::default(),
        }
    }
}

impl AppConfig {
    /// 默认配置文件路径：可执行文件同目录。
    ///
    /// 用 exe 同目录而不是 `%APPDATA%`，是为了让「绿色版」真的绿色 ——
    /// 整个文件夹拷走就带着配置，不用去别处找。
    pub fn default_path() -> PathBuf {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join(DEFAULT_FILE_NAME)))
            .unwrap_or_else(|| PathBuf::from(DEFAULT_FILE_NAME))
    }

    /// 从磁盘加载。**任何失败都回退默认值并给出提示，不返回错误。**
    ///
    /// 文件不存在属于正常情况（首次运行），不应视为错误，`note` 为 `None`。
    pub fn load(path: &Path) -> (Self, Option<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_json(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // 首次运行：静默使用默认值
                (Self::default(), None)
            }
            Err(e) => (
                Self::default(),
                Some(format!(
                    "读取配置 {} 失败（{e}），已使用默认值",
                    path.display()
                )),
            ),
        }
    }

    /// 原子保存。
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| format!("创建配置目录 {} 失败: {e}", dir.display()))?;
            }
        }
        let text = self.to_json();
        let tmp = {
            let mut s = path.as_os_str().to_os_string();
            s.push(".tmp");
            PathBuf::from(s)
        };
        std::fs::write(&tmp, text.as_bytes())
            .map_err(|e| format!("写临时配置 {} 失败: {e}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .map_err(|e| format!("替换配置 {} 失败: {e}", path.display()))?;
        Ok(())
    }

    /// 从 JSON 解析，失败回退默认值。
    ///
    /// 会**自动识别并迁移旧格式**：0.1.0 早期版本的配置文件顶层直接就是
    /// `Settings`（含 `angle` / `g_meter` / `theme_name` 等键）。如果按新格式
    /// 解析，那些键会被 serde 当作「未知字段」静默忽略，用户辛苦调好的外观
    /// 参数会全部丢失。因此这里先探测旧格式，命中则整体搬进 `hud` 段。
    pub fn from_json(s: &str) -> (Self, Option<String>) {
        // ---- 旧格式探测（顶层没有 `hud`，但有 Settings 的特征字段）----
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(s) {
            if value.is_object()
                && value.get("hud").is_none()
                && (value.get("angle").is_some()
                    || value.get("g_meter").is_some()
                    || value.get("theme_name").is_some()
                    || value.get("input_panel").is_some())
            {
                if let Ok(mut settings) = serde_json::from_value::<Settings>(value) {
                    settings.sanitize();
                    let cfg = AppConfig {
                        hud: settings,
                        ..Default::default()
                    };
                    return (
                        cfg,
                        Some(
                            "检测到旧版（仅外观参数）配置，已自动迁移到新格式的 hud 段".to_string(),
                        ),
                    );
                }
            }
        }

        match serde_json::from_str::<AppConfig>(s) {
            Ok(mut v) => {
                let fixed = v.sanitize();
                let note = if fixed {
                    Some("部分配置项超出合法范围，已自动修正".to_string())
                } else {
                    None
                };
                (v, note)
            }
            Err(e) => (
                Self::default(),
                Some(format!("配置解析失败，已回退到默认值: {e}")),
            ),
        }
    }

    /// 序列化成带缩进的 JSON。
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// 校验三个子配置并修正版本号。
    ///
    /// 返回 `true` 表示发生了修正。
    pub fn sanitize(&mut self) -> bool {
        let before = (
            self.hud.clone(),
            self.relay.clone(),
            self.logging.clone(),
            self.apps.clone(),
            self.version,
        );
        self.hud.sanitize();
        self.relay.sanitize();
        self.logging.sanitize();
        self.apps.sanitize();
        if self.version == 0 {
            self.version = APP_CONFIG_VERSION;
        }
        // 子配置版本也应跟随
        if self.hud.version == 0 {
            self.hud.version = SETTINGS_VERSION;
        }
        before
            != (
                self.hud.clone(),
                self.relay.clone(),
                self.logging.clone(),
                self.apps.clone(),
                self.version,
            )
    }

    /// 用本配置初始化全局日志器，并记录一条启动信息。
    pub fn init_logging(&self, who: &str) {
        let logger = crate::logging::Logger::init(&self.logging);
        logger.info(&format!(
            "=== {who} 启动 === 配置版本 {} hud={} relay={} 目标数={} 日志级别={}",
            self.version,
            self.hud.network.endpoint(),
            self.relay.endpoint(),
            self.relay.enabled_count(),
            self.logging.level.as_str()
        ));
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    #[test]
    fn 默认配置合法() {
        let mut c = AppConfig::default();
        assert!(!c.sanitize(), "默认配置不应需要修正");
        assert_eq!(c.version, APP_CONFIG_VERSION);
        assert!(c.hud.language == crate::settings::Language::Zh);
        assert_eq!(c.hud.network.listen_port, 5302);
        assert_eq!(c.relay.listen_port, 5301);
    }

    #[test]
    fn json往返无损() {
        let mut c = AppConfig::default();
        c.hud.language = crate::settings::Language::En;
        c.hud.network.listen_port = 5402;
        c.relay.add_target("192.168.1.9", 8001, "第二台机器");
        c.logging.enabled = true;
        let json = c.to_json();
        let (back, note) = AppConfig::from_json(&json);
        assert!(note.is_none(), "合法 JSON 不应有提示: {note:?}");
        assert_eq!(back, c);
    }

    #[test]
    fn 缺失字段全部取默认值() {
        let (c, note) = AppConfig::from_json("{}");
        assert!(note.is_none());
        assert_eq!(c, AppConfig::default());
    }

    #[test]
    fn 部分字段存在时其余取默认值() {
        let (c, _) = AppConfig::from_json(r#"{"hud":{"network":{"listen_port":5555}}}"#);
        assert_eq!(c.hud.network.listen_port, 5555);
        assert_eq!(c.hud.network.listen_ip, "127.0.0.1");
        assert_eq!(c.relay, RelayConfig::default());
    }

    #[test]
    fn 损坏的json回退默认值() {
        for bad in ["", "{", "null", "not json", "[1,2,3]"] {
            let (c, note) = AppConfig::from_json(bad);
            assert!(note.is_some(), "损坏输入 `{bad}` 应产生提示");
            // null / [] 会被 serde 当作“整体缺失”，仍应得到合法默认值
            let mut expect = AppConfig::default();
            expect.sanitize();
            assert_eq!(c, expect, "`{bad}` 应回退到默认配置");
        }
    }

    #[test]
    fn 原子保存后读回一致() {
        let dir = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("cfgsave");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("fh6-hud.config.json");
        let mut c = AppConfig::default();
        c.relay.add_target("127.0.0.1", 8001, "SimHub");
        c.save(&path).expect("保存应成功");

        assert!(path.exists(), "配置文件应存在");
        let tmp = {
            let mut s = path.as_os_str().to_os_string();
            s.push(".tmp");
            PathBuf::from(s)
        };
        assert!(!tmp.exists(), "临时文件应已被 rename 掉");

        let (back, note) = AppConfig::load(&path);
        assert!(note.is_none());
        assert_eq!(back, c);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 文件不存在时静默用默认值() {
        let path = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("不存在的目录")
            .join("none.json");
        let (c, note) = AppConfig::load(&path);
        assert!(note.is_none(), "文件不存在不算错误");
        assert_eq!(c, AppConfig::default());
    }

    #[test]
    fn 损坏的文件回退默认值并提示() {
        let dir = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("cfgbad");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.json");
        std::fs::write(&path, "{ 这不是 json").unwrap();
        let (c, note) = AppConfig::load(&path);
        assert!(note.is_some(), "损坏文件应给出提示");
        assert_eq!(c.hud, Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 越界值被修正并提示() {
        let (c, note) = AppConfig::from_json(
            r#"{"hud":{"global_scale":999},"relay":{"listen_port":0,"targets":[]},"logging":{"max_bytes":1}}"#,
        );
        assert!(note.is_some(), "应提示发生了修正");
        assert_eq!(c.hud.global_scale, 4.0);
        assert_eq!(c.relay.listen_port, 5301);
        assert_eq!(c.logging.max_bytes, 4096);
    }

    #[test]
    fn 默认路径以默认文件名结尾() {
        let p = AppConfig::default_path();
        assert_eq!(
            p.file_name().and_then(|s| s.to_str()),
            Some(DEFAULT_FILE_NAME)
        );
    }

    #[test]
    fn 子配置版本为0时被补上() {
        let (c, _) = AppConfig::from_json(r#"{"version":0,"hud":{"version":0}}"#);
        assert_eq!(c.version, APP_CONFIG_VERSION);
        assert_eq!(c.hud.version, SETTINGS_VERSION);
    }

    #[test]
    fn 旧版仅外观参数的配置被自动迁移() {
        // 0.1.0 早期的配置文件格式：顶层就是 Settings
        let legacy = r#"{
            "version": 1,
            "theme_name": "street",
            "global_scale": 1.5,
            "angle": {"cx": 800.0, "cy": 500.0, "radius": 300.0},
            "input_panel": {"x": 10.0, "y": 20.0}
        }"#;
        let (c, note) = AppConfig::from_json(legacy);
        let note = note.expect("迁移应给出提示");
        assert!(note.contains("旧版"), "提示应说明是旧版迁移: {note}");
        // 关键：旧配置里的值必须保留，而不是被默认值覆盖
        assert_eq!(c.hud.theme_name, "street");
        assert_eq!(c.hud.global_scale, 1.5);
        assert_eq!(c.hud.angle.cx, 800.0);
        assert_eq!(c.hud.angle.radius, 300.0);
        assert_eq!(c.hud.input_panel.x, 10.0);
        // 其余段仍取默认值
        assert_eq!(c.relay, RelayConfig::default());
    }

    #[test]
    fn 新格式不会被误判为旧格式() {
        let new = r#"{"version":1,"hud":{"theme_name":"x"},"relay":{"listen_port":5400}}"#;
        let (c, note) = AppConfig::from_json(new);
        assert!(note.is_none(), "新格式不应有迁移提示: {note:?}");
        assert_eq!(c.hud.theme_name, "x");
        assert_eq!(c.relay.listen_port, 5400);
    }
}
