//! FH6 遥测 UDP 分发器的可测试内核。
//!
//! 二进制入口在 `main.rs`，这里只提供「绑定、转发、统计」的纯逻辑，
//! 以便集成测试能够在**进程内**驱动真实的 UDP 收发 —— 不需要启动子进程，
//! 也不需要连着游戏。
//!
//! 设计要点见 [`Relay::bind`] 与 [`Relay::run`] 的文档。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

/// 接收缓冲区大小。
///
/// 数据包固定 324 字节，取 2048 留足余量（转发链路上可能附加自定义头）。
/// 用固定栈数组，保证热路径零堆分配。
pub const RECV_BUF_SIZE: usize = 2048;

/// 统计信息快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    /// 累计收到的数据包数（含短包）。
    pub packets: u64,
    /// 累计收到的字节数。
    pub bytes: u64,
    /// 其中长度不足 324 字节的包数。
    pub short_packets: u64,
    /// 最近一次发包来源。
    pub last_sender: Option<SocketAddr>,
}

/// 单个转发目标的统计。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TargetStats {
    /// 发送成功次数。
    pub sent: u64,
    /// 发送失败次数（例如目标端口没有进程监听）。
    pub failed: u64,
}

/// 转发目标。
#[derive(Debug, Clone)]
struct Target {
    addr: SocketAddr,
    label: String,
    stats: TargetStats,
}

/// 遥测分发器。
///
/// # 为什么不设置 `SO_REUSEADDR`
///
/// 本 relay 要求**独占**监听端口。如果端口已被占用（例如误把游戏端口设成
/// 5300 而飞智正在监听），我们希望**立刻报错**，而不是"绑定成功但收不到包"
/// —— 后者会让人排查半天。
///
/// 这条规则有实测依据（Windows 10 19045）：
/// * 两个套接字都设 `SO_REUSEADDR` → 都能绑定成功，但单播包**只投递给第一个
///   绑定者**，第二个永远收不到；
/// * 只有一个设了 `SO_REUSEADDR` → 后绑定者直接失败。
pub struct Relay {
    socket: UdpSocket,
    targets: Vec<Target>,
    stats: Stats,
    buf: [u8; RECV_BUF_SIZE],
}

impl Relay {
    /// 绑定监听地址并登记转发目标。
    pub fn bind(listen: SocketAddr, targets: &[(String, u16)]) -> io::Result<Self> {
        let socket = UdpSocket::bind(listen)?;
        Self::from_socket(socket, targets)
    }

    /// 接管一个**已经绑定好**的套接字。
    ///
    /// # 为什么需要这个入口
    ///
    /// 「先 `bind(0)` 拿到端口号 → `drop` → 再绑同一个号」这种写法存在竞态窗口：
    /// 端口一旦释放，别的进程/线程就可能抢走，随后绑定失败。
    /// 在多线程测试里这个窗口会真实触发（本项目就遇到了：
    /// 单独跑 8 个用例全过，并行跑时偶发失败）。
    ///
    /// 由调用方持有套接字并直接交给 relay，可以**彻底消除**这个窗口：
    /// 端口从绑定到交接之间从未进入"空闲"状态。
    ///
    /// 注意这不影响 [`Relay::bind`] 的严格性 —— `SO_REUSEADDR` 的语义是
    /// **绑定时刻**决定的，本函数不会放宽任何东西。
    pub fn from_socket(socket: UdpSocket, targets: &[(String, u16)]) -> io::Result<Self> {
        let mut resolved = Vec::with_capacity(targets.len());
        for (host, port) in targets {
            // 这里只做字面量解析；主机名解析在调用方完成（见 main.rs 的 cli）。
            let addr: SocketAddr = format!("{host}:{port}")
                .parse()
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("{e}")))?;
            resolved.push(Target {
                addr,
                label: format!("{host}:{port}"),
                stats: TargetStats::default(),
            });
        }

        Ok(Self {
            socket,
            targets: resolved,
            stats: Stats::default(),
            buf: [0u8; RECV_BUF_SIZE],
        })
    }

    /// 实际绑定的本地地址（绑定端口传 0 时可用于获取系统分配的端口）。
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// 设置读取超时。
    ///
    /// 生产环境用阻塞模式（无数据时 CPU 占用为 0）；测试与需要定期做其他
    /// 工作的场景用超时模式。
    pub fn set_read_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
        self.socket.set_read_timeout(dur)
    }

    /// 转发目标列表（只读，用于打印启动信息）。
    pub fn targets(&self) -> impl Iterator<Item = (&str, SocketAddr)> {
        self.targets.iter().map(|t| (t.label.as_str(), t.addr))
    }

    /// 当前统计。
    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// 各转发目标的统计。
    pub fn target_stats(&self) -> Vec<(&str, TargetStats)> {
        self.targets
            .iter()
            .map(|t| (t.label.as_str(), t.stats))
            .collect()
    }

    /// 接收一次数据包并原样转发给全部目标。
    ///
    /// 返回本次收到的字节数；`Ok(None)` 表示读取超时（无数据）。
    ///
    /// **不解析数据包**：转发必须逐字节透明，否则下游（飞智的扳机模块）
    /// 拿到的字节就和游戏发出的不一致了。
    pub fn pump_once(&mut self) -> io::Result<Option<usize>> {
        let (len, from) = match self.socket.recv_from(&mut self.buf) {
            Ok(v) => v,
            Err(e) => {
                if is_timeout(&e) {
                    return Ok(None);
                }
                // 收到 ICMP 端口不可达后，Windows 会让下次 recv 返回
                // ConnectionReset。某个转发目标没在监听时这是预期行为，
                // 不能因此让 relay 退出，否则游戏一发第一个包 relay 就死。
                if is_transient(&e) {
                    return Ok(None);
                }
                return Err(e);
            }
        };

        self.stats.packets += 1;
        self.stats.bytes += len as u64;
        self.stats.last_sender = Some(from);
        if len < fh6_core::PACKET_SIZE {
            self.stats.short_packets += 1;
        }

        for t in self.targets.iter_mut() {
            match self.socket.send_to(&self.buf[..len], t.addr) {
                Ok(_) => t.stats.sent += 1,
                Err(_) => t.stats.failed += 1,
            }
        }

        Ok(Some(len))
    }

    /// 持续转发，直到 `should_stop` 返回 `true`。
    ///
    /// `should_stop` 在每次循环（含超时）后调用一次，因此可以响应停止信号。
    pub fn run<F>(&mut self, mut should_stop: F) -> io::Result<()>
    where
        F: FnMut(&Stats) -> bool,
    {
        loop {
            self.pump_once()?;
            if should_stop(&self.stats) {
                return Ok(());
            }
        }
    }
}

/// 判断错误是否为「读取超时」。
fn is_timeout(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

/// 判断错误是否为可以安全忽略的瞬时错误。
fn is_transient(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionRefused
            | io::ErrorKind::Interrupted
    )
}

/// 解析 `主机:端口`，返回 (主机, 端口)。
///
/// 支持 IPv6 字面量写法 `[::1]:5300`。
pub fn parse_host_port(s: &str) -> Result<(String, u16), String> {
    let s = s.trim();
    let idx = s
        .rfind(':')
        .ok_or_else(|| format!("`{s}` 缺少端口，正确写法如 127.0.0.1:5300"))?;
    let (host, port) = s.split_at(idx);
    let port: u16 = port[1..]
        .parse()
        .map_err(|e| format!("`{s}` 的端口不合法: {e}"))?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() {
        return Err(format!("`{s}` 缺少主机名"));
    }
    Ok((host.to_string(), port))
}

/// 计算速率指标，用于日志输出。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rate {
    /// 每秒包数。
    pub packets_per_sec: f32,
    /// 每秒千比特。
    pub kbps: f32,
}

impl Rate {
    /// 由「累计包数 / 字节数」和「经过时间」计算。
    pub fn from(stats: &Stats, elapsed: Duration) -> Self {
        let secs = elapsed.as_secs_f32();
        if secs <= 0.0 {
            return Self {
                packets_per_sec: 0.0,
                kbps: 0.0,
            };
        }
        Self {
            packets_per_sec: stats.packets as f32 / secs,
            kbps: (stats.bytes as f32 * 8.0 / 1000.0) / secs,
        }
    }
}

/// 供上层格式化的日志辅助：判断是否该输出一次统计。
pub fn should_log(last_log: Instant, interval: Duration) -> bool {
    last_log.elapsed() >= interval
}
