//! GUI 的应用路径与启动行为配置。
//!
//! 放在同一份 `AppConfig` 里而不是 GUI 私有的小文件，理由与 [`crate::app`] 相同：
//! 用户拷走整个文件夹时配置跟着走；命令行工具也能读到「该启动哪两个 exe」。

use serde::{Deserialize, Serialize};

/// 两个子进程的可执行文件路径与自动启动开关。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppsConfig {
    /// `fh6-hud.exe` 的路径。空字符串表示**自动查找**。
    pub hud_exe: String,
    /// `fh6-relay.exe` 的路径。空字符串表示**自动查找**。
    pub relay_exe: String,
    /// GUI 启动时是否自动拉起这两个进程。
    pub auto_start: bool,
    /// 关闭窗口时是否最小化到托盘（而不是退出进程）。
    ///
    /// 默认 `true`：用户要的是「后台常驻」，误点关闭不应该把 HUD 一起关掉。
    pub close_to_tray: bool,
}

impl Default for AppsConfig {
    fn default() -> Self {
        Self {
            hud_exe: String::new(),
            relay_exe: String::new(),
            auto_start: false,
            close_to_tray: true,
        }
    }
}

impl AppsConfig {
    /// 校验并规范化。
    ///
    /// 路径只做长度与空白处理：**不检查文件是否存在** —— 配置可能来自另一台
    /// 机器或尚未编译的目录，过早报错反而妨碍用户先保存配置。
    pub fn sanitize(&mut self) -> bool {
        let before = self.clone();
        self.hud_exe = normalize_path(&self.hud_exe);
        self.relay_exe = normalize_path(&self.relay_exe);
        *self != before
    }
}

/// 去空白并在超长时截断。
fn normalize_path(s: &str) -> String {
    let t = s.trim();
    if t.len() > 512 {
        t.chars().take(512).collect()
    } else {
        t.to_string()
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    #[test]
    fn 默认值合法() {
        let mut a = AppsConfig::default();
        assert!(!a.sanitize());
        assert!(a.hud_exe.is_empty(), "空路径表示自动查找");
        assert!(a.close_to_tray, "默认关闭窗口应收起到托盘");
    }

    #[test]
    fn 路径去空白() {
        let mut a = AppsConfig {
            hud_exe: "  C:/x/fh6-hud.exe  ".to_string(),
            ..Default::default()
        };
        assert!(a.sanitize());
        assert_eq!(a.hud_exe, "C:/x/fh6-hud.exe");
    }

    #[test]
    fn 超长路径被截断() {
        let mut a = AppsConfig {
            relay_exe: "x".repeat(600),
            ..Default::default()
        };
        a.sanitize();
        assert_eq!(a.relay_exe.chars().count(), 512);
    }

    #[test]
    fn json往返无损() {
        let a = AppsConfig {
            hud_exe: "C:/a/fh6-hud.exe".to_string(),
            relay_exe: "C:/a/fh6-relay.exe".to_string(),
            auto_start: true,
            close_to_tray: false,
        };
        let json = serde_json::to_string(&a).unwrap();
        let back: AppsConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back, a);
    }
}
