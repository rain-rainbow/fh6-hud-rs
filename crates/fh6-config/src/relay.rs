//! relay（UDP 分发器）的配置。
//!
//! # 为什么把「分发目标」做成列表
//!
//! 原先的目标列表是编译期常量（5300 飞智 + 5302 HUD）。但实际使用中
//! 目标会变：可能同时喂 SimHub、录数据脚本、第二台机器；也可能暂时关掉
//! 某一项。因此改成**有序、可增删、可单独禁用**的列表，由 GUI 编辑。
//!
//! # 一个必须防住的危险配置
//!
//! 如果某个分发目标的地址与端口**等于 relay 自己的监听端点**，那么
//! relay 转发出去的包会被它自己再收到一次，再次转发……形成**自激包风暴**，
//! 瞬间打满 CPU。因此 [`RelayConfig::sanitize`] 会直接**剔除**这类目标，
//! [`RelayConfig::conflicts`] 则供界面提示用户。这条有专门的单元测试。

use serde::{Deserialize, Serialize};

/// 分发目标数量上限。
///
/// 不是技术限制，而是防止畸形配置（或界面误操作）让 relay 每收一个包就
/// 扇出上千次。32 个目标已经远超实际需求。
pub const MAX_TARGETS: usize = 32;

/// relay 默认监听端口。
pub const DEFAULT_LISTEN_PORT: u16 = 5301;

/// 一个分发目标。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RelayTarget {
    /// 是否启用。禁用后保留在配置里但不参与转发。
    pub enabled: bool,
    /// 目标主机（IP 或可解析的域名）。
    pub host: String,
    /// 目标端口。
    pub port: u16,
    /// 显示名称，仅用于日志与界面。
    pub label: String,
}

impl Default for RelayTarget {
    fn default() -> Self {
        Self {
            enabled: true,
            host: "127.0.0.1".to_string(),
            port: 5300,
            label: String::new(),
        }
    }
}

impl RelayTarget {
    /// 新建一个启用的目标。
    pub fn new(host: &str, port: u16, label: &str) -> Self {
        Self {
            enabled: true,
            host: host.to_string(),
            port,
            label: label.to_string(),
        }
    }

    /// `host:port` 形式。
    pub fn endpoint(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// 用于日志显示的标签（空标签回退到 endpoint）。
    pub fn display(&self) -> String {
        if self.label.trim().is_empty() {
            self.endpoint()
        } else {
            format!("{} ({})", self.label, self.endpoint())
        }
    }

    /// 校验并规范化单个目标。
    ///
    /// 返回 `false` 表示该目标**不可用**（主机为空或端口为 0），调用方应剔除。
    pub fn sanitize(&mut self) -> bool {
        let host = self.host.trim();
        if host.is_empty() || host.len() > 255 || self.port == 0 {
            self.enabled = false;
            return false;
        }
        self.host = host.to_string();
        if self.label.chars().count() > 32 {
            self.label = self.label.chars().take(32).collect();
        }
        true
    }
}

/// relay 的完整配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RelayConfig {
    /// 监听地址。默认 `127.0.0.1`（只接本机游戏）。
    pub listen_ip: String,
    /// 监听端口。默认 5301（游戏内「数据输出IP端口」应填这个）。
    pub listen_port: u16,
    /// 分发目标（有序，界面按此顺序显示）。
    pub targets: Vec<RelayTarget>,
    /// 是否打印每个包（调试用，会显著增加 I/O）。
    pub verbose: bool,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            listen_ip: "127.0.0.1".to_string(),
            listen_port: DEFAULT_LISTEN_PORT,
            targets: vec![
                // 顺序有意把飞智放第一：万一将来需要按顺序降级，
                // 优先保证自适应扳机不失效。
                RelayTarget::new("127.0.0.1", 5300, "飞智自适应扳机"),
                RelayTarget::new("127.0.0.1", 5302, "HUD"),
            ],
            verbose: false,
        }
    }
}

impl RelayConfig {
    /// 监听端点字符串。
    pub fn endpoint(&self) -> String {
        format!("{}:{}", self.listen_ip, self.listen_port)
    }

    /// 判断某个目标是否与给定监听端点重合（自转发）。
    ///
    /// 写成自由函数而不是 &self 方法，是为了在 iter_mut() 遍历 targets 时
    /// 不产生「同时可变借用 targets、不可变借用 self」的借用冲突。
    fn target_is_self(listen_ip: &str, listen_port: u16, t: &RelayTarget) -> bool {
        if t.port != listen_port {
            return false;
        }
        let a = t.host.trim().to_ascii_lowercase();
        let b = listen_ip.trim().to_ascii_lowercase();
        // 回环等价：127.0.0.1 / localhost / ::1 视为同一地址
        let loopbacks = ["127.0.0.1", "localhost", "::1", "[::1]"];
        let a_is_loop = loopbacks.contains(&a.as_str());
        let b_is_loop = loopbacks.contains(&b.as_str());
        // 监听 0.0.0.0 时，任何以本机地址为目标的同端口转发都会回到自己
        (a_is_loop && b_is_loop) || a == b || a == "0.0.0.0" || b == "0.0.0.0"
    }

    /// 是否与监听端点相同（自转发）。
    fn is_self_target(&self, t: &RelayTarget) -> bool {
        Self::target_is_self(&self.listen_ip, self.listen_port, t)
    }

    /// 找出会与监听端点冲突（导致自转发）的目标，返回其描述。
    ///
    /// 供界面提示用户；[`RelayConfig::sanitize`] 会自动把它们禁用。
    pub fn conflicts(&self) -> Vec<String> {
        self.targets
            .iter()
            .enumerate()
            .filter(|(_, t)| t.enabled && self.is_self_target(t))
            .map(|(i, t)| format!("#{} {}", i + 1, t.endpoint()))
            .collect()
    }

    /// 校验并规范化整个配置。
    ///
    /// 返回 `true` 表示发生了修正。修正包括：
    /// * 监听地址空/异常 → `127.0.0.1`；端口 0 → 5301；
    /// * 目标数超过 [`MAX_TARGETS`] → 截断；
    /// * 无效目标（空主机、端口 0）→ 剔除；
    /// * **会自转发的目标 → 禁用**（避免包风暴）。
    pub fn sanitize(&mut self) -> bool {
        let before = self.clone();

        let ip = self.listen_ip.trim();
        if ip.is_empty() || ip.len() > 64 {
            self.listen_ip = "127.0.0.1".to_string();
        } else {
            self.listen_ip = ip.to_string();
        }
        if self.listen_port == 0 {
            self.listen_port = DEFAULT_LISTEN_PORT;
        }

        if self.targets.len() > MAX_TARGETS {
            self.targets.truncate(MAX_TARGETS);
        }

        // 先剔除无效项，再处理自转发
        self.targets.retain_mut(|t| t.sanitize());
        let (lip, lport) = (self.listen_ip.clone(), self.listen_port);
        for t in self.targets.iter_mut() {
            if t.enabled && Self::target_is_self(&lip, lport, t) {
                t.enabled = false;
            }
        }

        *self != before
    }

    /// 已启用的目标，转换成 `(host, port)` 列表，供 relay 直接使用。
    pub fn enabled_targets(&self) -> Vec<(String, u16)> {
        self.targets
            .iter()
            .filter(|t| t.enabled && t.port != 0 && !t.host.trim().is_empty())
            .map(|t| (t.host.clone(), t.port))
            .collect()
    }

    /// 已启用的目标数量。
    pub fn enabled_count(&self) -> usize {
        self.targets
            .iter()
            .filter(|t| t.enabled && t.port != 0 && !t.host.trim().is_empty())
            .count()
    }

    /// 追加一个目标；超过上限时返回 `false`。
    pub fn add_target(&mut self, host: &str, port: u16, label: &str) -> bool {
        if self.targets.len() >= MAX_TARGETS {
            return false;
        }
        self.targets.push(RelayTarget::new(host, port, label));
        self.sanitize();
        true
    }

    /// 删除第 `index` 个目标；越界返回 `false`。
    pub fn remove_target(&mut self, index: usize) -> bool {
        if index >= self.targets.len() {
            return false;
        }
        self.targets.remove(index);
        true
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    #[test]
    fn 默认配置合法且不触发自转发() {
        let mut c = RelayConfig::default();
        assert!(!c.sanitize(), "默认配置不应需要修正");
        assert_eq!(c.listen_port, 5301);
        assert_eq!(c.enabled_count(), 2);
        assert!(c.conflicts().is_empty(), "默认目标不应与监听端点冲突");
        let t = c.enabled_targets();
        assert_eq!(t[0], ("127.0.0.1".to_string(), 5300));
        assert_eq!(t[1], ("127.0.0.1".to_string(), 5302));
    }

    #[test]
    fn 目标与监听端口相同时会被禁用而不是造成包风暴() {
        // 这是最危险的一种误配置：relay 转发给自己
        let mut c = RelayConfig {
            listen_port: 5301,
            ..Default::default()
        };
        c.targets
            .push(RelayTarget::new("127.0.0.1", 5301, "错误目标"));
        let changed = c.sanitize();
        assert!(changed, "应报告发生了修正");
        // 该目标应被禁用
        let bad = c.targets.iter().find(|t| t.port == 5301).unwrap();
        assert!(!bad.enabled, "自转发目标必须被禁用");
        assert_eq!(c.enabled_count(), 2, "只有原来那两个目标仍启用");
        assert!(c.enabled_targets().iter().all(|(_, p)| *p != 5301));
    }

    #[test]
    fn 回环地址的各种写法都被识别为自转发() {
        for host in ["127.0.0.1", "localhost", "::1", "[::1]", "LOCALHOST"] {
            let mut c = RelayConfig {
                listen_ip: "127.0.0.1".to_string(),
                listen_port: 5301,
                ..Default::default()
            };
            c.targets.clear();
            c.targets.push(RelayTarget::new(host, 5301, ""));
            c.sanitize();
            assert_eq!(c.enabled_count(), 0, "`{host}` 应被识别为自转发");
        }
    }

    #[test]
    fn 监听在0000时同端口的目标也被拦下() {
        let mut c = RelayConfig {
            listen_ip: "0.0.0.0".to_string(),
            listen_port: 5301,
            ..Default::default()
        };
        c.targets.clear();
        c.targets.push(RelayTarget::new("192.168.1.10", 5301, ""));
        c.sanitize();
        assert_eq!(c.enabled_count(), 0, "监听全部接口时同端口仍是自转发");
    }

    #[test]
    fn 不同端口的同地址目标不受影响() {
        let mut c = RelayConfig::default();
        c.targets.clear();
        c.targets.push(RelayTarget::new("127.0.0.1", 9999, "其他"));
        c.sanitize();
        assert_eq!(c.enabled_count(), 1);
    }

    #[test]
    fn 空主机与零端口目标被剔除() {
        let mut c = RelayConfig::default();
        c.targets.clear();
        c.targets.push(RelayTarget::new("", 5300, "空主机"));
        c.targets.push(RelayTarget::new("127.0.0.1", 0, "零端口"));
        c.targets.push(RelayTarget::new("127.0.0.1", 5300, "正常"));
        c.sanitize();
        assert_eq!(c.targets.len(), 1, "无效目标应被剔除");
        assert_eq!(c.targets[0].label, "正常");
    }

    #[test]
    fn 目标数量超过上限时被截断() {
        let mut c = RelayConfig::default();
        c.targets.clear();
        for i in 0..50u16 {
            c.targets.push(RelayTarget::new("127.0.0.1", 6000 + i, ""));
        }
        c.sanitize();
        assert_eq!(c.targets.len(), MAX_TARGETS);
    }

    #[test]
    fn 增删目标正常() {
        let mut c = RelayConfig::default();
        assert_eq!(c.targets.len(), 2);
        assert!(c.add_target("127.0.0.1", 8001, "SimHub"));
        assert_eq!(c.targets.len(), 3);
        assert!(c.remove_target(2));
        assert_eq!(c.targets.len(), 2);
        assert!(!c.remove_target(99), "越界删除应返回 false");
    }

    #[test]
    fn 新增自转发目标会被立即禁用() {
        let mut c = RelayConfig::default();
        c.add_target("127.0.0.1", c.listen_port, "自转发");
        let t = c.targets.last().unwrap();
        assert!(!t.enabled, "add_target 内部应已 sanitize");
    }

    #[test]
    fn 禁用的目标不参与转发() {
        let mut c = RelayConfig::default();
        c.targets[0].enabled = false;
        assert_eq!(c.enabled_count(), 1);
        assert_eq!(c.enabled_targets()[0].1, 5302);
    }

    #[test]
    fn 监听地址与端口异常时回退() {
        let mut c = RelayConfig {
            listen_ip: "   ".to_string(),
            listen_port: 0,
            ..Default::default()
        };
        c.sanitize();
        assert_eq!(c.listen_ip, "127.0.0.1");
        assert_eq!(c.listen_port, DEFAULT_LISTEN_PORT);
        assert_eq!(c.endpoint(), "127.0.0.1:5301");
    }

    #[test]
    fn 显示名在空标签时回退到endpoint() {
        let t = RelayTarget::new("127.0.0.1", 5300, "");
        assert_eq!(t.display(), "127.0.0.1:5300");
        let t2 = RelayTarget::new("127.0.0.1", 5300, "飞智");
        assert!(t2.display().contains("飞智"));
        assert!(t2.display().contains("127.0.0.1:5300"));
    }

    #[test]
    fn json往返无损() {
        let mut c = RelayConfig::default();
        c.add_target("192.168.1.50", 8001, "第二台机器");
        c.verbose = true;
        let json = serde_json::to_string_pretty(&c).unwrap();
        let back: RelayConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn 缺失字段用默认值补齐() {
        let c: RelayConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(c, RelayConfig::default());
        let c2: RelayConfig = serde_json::from_str(r#"{"listen_port": 5400}"#).unwrap();
        assert_eq!(c2.listen_port, 5400);
        assert!(c2.targets.len() >= 2, "缺失的 targets 应取默认值");
    }
}
