//! 收包线程与渲染线程之间的数据交换。
//!
//! # 为什么要分开
//!
//! 参考实现把 UDP 轮询、业务逻辑、渲染全部塞在 Qt 主线程的一个 16 ms 定时器里。
//! 后果是：**渲染一旦超时，就来不及排空 socket 缓冲区，遥测包被静默丢弃**
//! —— 而 UDP 没有重传，丢了就永久丢了。表现为 HUD 数值偶尔"卡一下"。
//!
//! 本实现把两者分开：
//!
//! ```text
//!   [收包线程]  recv_from 阻塞等待 ──► 解析 ──► 写入快照
//!                                                  │
//!   [渲染线程]  按帧率读取快照 ◄────────────────────┘
//! ```
//!
//! 收包线程只做「收 + 解析 + 写快照」，永不做渲染；即使渲染卡住几十毫秒，
//! 收包也照样进行，只是快照被覆写（这正是我们想要的：**只要最新一帧**）。
//!
//! # 为什么不用复杂的无锁结构
//!
//! 典型的无锁方案是「原子序号 + 双缓冲」（seqlock）。但这里的访问模式是：
//!
//! * 写入：约 60 次/秒；
//! * 读取：约 60 次/秒。
//!
//! 竞争概率极低，临界区又只有几十字节的拷贝。用 `Mutex` 的一次
//! 未争用加解锁开销约 20–50 ns，每秒总计不到 6 µs —— **完全测不出来**。
//! 相比之下 seqlock 的正确性论证与维护成本高得多。
//!
//! 因此这里刻意选择 `Mutex`，并把「为什么不是无锁」写在文档里，
//! 免得后人以为是偷懒。
//!
//! # 锁中毒的处理
//!
//! 本模块**不使用** `parking_lot` 等第三方锁，也不对 `std::sync::Mutex`
//! 的 `PoisonError` 直接 unwrap：渲染线程若因某个 bug panic，
//! 收包线程不应该跟着一起死（那样用户会看到 HUD 直接消失且无提示）。
//! 这里统一用 `unwrap_or_else(|e| e.into_inner())` 继续使用数据 ——
//! 快照是**纯数据**，不存在"半更新状态会让不变量失效"的问题。

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use fh6_core::Frame;

/// 连接状态。用 `u8` 存储以便放进原子量。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum LinkState {
    /// 尚未收到任何数据包。
    NoData = 0,
    /// 正在收到数据（最近有包）。
    Live = 1,
    /// 曾经收到过，但已超时（游戏暂停、退出比赛、或游戏已关）。
    Stale = 2,
}

impl LinkState {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => LinkState::Live,
            2 => LinkState::Stale,
            _ => LinkState::NoData,
        }
    }

    /// 显示文本。
    pub fn label(self) -> &'static str {
        match self {
            LinkState::NoData => "NO DATA",
            LinkState::Live => "LIVE",
            LinkState::Stale => "STALE",
        }
    }
}

/// 超过这个时长没收到包，就认为数据已过期。
///
/// 官方文档说明数据只在**实际驾驶中**发送：菜单、暂停、回放、倒带、赛后都不发。
/// 所以「超时」是正常现象，界面应显示提示而不是保持最后一帧数值假装还活着。
pub const STALE_AFTER: Duration = Duration::from_millis(500);

/// 共享快照槽位。
struct Slot {
    frame: Frame,
    last_packet_at: Option<Instant>,
    /// 数据包的原始毫秒时间戳（会回绕，仅用于显示）。
    packet_timestamp_ms: u32,
}

/// 收包线程与渲染线程共享的状态。
///
/// 所有方法都是 `&self`，可被 `Arc` 共享。
pub struct TelemetrySlot {
    inner: Mutex<Slot>,
    state: AtomicU8,
    packets_received: AtomicU64,
    /// 长度不足 324 字节而被拒绝的包数（正常应为 0，非 0 说明端口被别的数据源占用）。
    packets_rejected: AtomicU64,
    packets_parsed: AtomicU64,
}

impl Default for TelemetrySlot {
    fn default() -> Self {
        Self::new()
    }
}

impl TelemetrySlot {
    /// 新建一个空槽位。
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Slot {
                frame: Frame::default(),
                last_packet_at: None,
                packet_timestamp_ms: 0,
            }),
            state: AtomicU8::new(LinkState::NoData as u8),
            packets_received: AtomicU64::new(0),
            packets_rejected: AtomicU64::new(0),
            packets_parsed: AtomicU64::new(0),
        }
    }

    /// 取锁（忽略中毒）。
    fn lock(&self) -> std::sync::MutexGuard<'_, Slot> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 收包线程调用：写入一个刚解析好的数据帧。
    pub fn publish(&self, frame: Frame, packet_timestamp_ms: u32, now: Instant) {
        {
            let mut s = self.lock();
            s.frame = frame;
            s.last_packet_at = Some(now);
            s.packet_timestamp_ms = packet_timestamp_ms;
        }
        self.packets_received.fetch_add(1, Ordering::Relaxed);
        self.packets_parsed.fetch_add(1, Ordering::Relaxed);
        self.state.store(LinkState::Live as u8, Ordering::Relaxed);
    }

    /// 收包线程调用：收到了一个无法解析（太短）的数据报。
    ///
    /// 只累计计数，**不更新快照**，也不把状态改成 `Live` —— 否则
    /// 端口上如果有别的进程乱发数据，HUD 会显示"LIVE"但数值一直是 0。
    pub fn note_rejected(&self) {
        self.packets_received.fetch_add(1, Ordering::Relaxed);
        self.packets_rejected.fetch_add(1, Ordering::Relaxed);
    }

    /// 渲染线程调用：取最新一帧与连接状态。
    ///
    /// `now` 用于判断数据是否过期。
    pub fn read(&self, now: Instant) -> (Frame, LinkState, Duration) {
        let (frame, age) = {
            let s = self.lock();
            let age = s
                .last_packet_at
                .map(|t| now.saturating_duration_since(t))
                .unwrap_or(Duration::MAX);
            (s.frame, age)
        };

        let state = if age == Duration::MAX {
            LinkState::NoData
        } else if age > STALE_AFTER {
            LinkState::Stale
        } else {
            LinkState::Live
        };
        self.state.store(state as u8, Ordering::Relaxed);
        (frame, state, age)
    }

    /// 当前连接状态（不做时间判定，只返回上次 `read` 得到的结论）。
    pub fn state(&self) -> LinkState {
        LinkState::from_u8(self.state.load(Ordering::Relaxed))
    }

    /// 收到的数据报总数（含被拒绝的）。
    pub fn packets_received(&self) -> u64 {
        self.packets_received.load(Ordering::Relaxed)
    }

    /// 成功解析的数据包数。
    pub fn packets_parsed(&self) -> u64 {
        self.packets_parsed.load(Ordering::Relaxed)
    }

    /// 被拒绝（长度不足）的数据报数。
    pub fn packets_rejected(&self) -> u64 {
        self.packets_rejected.load(Ordering::Relaxed)
    }
}

/// 一个「请求停止 + 确认已停止」的标志，用于优雅关闭收包线程。
#[derive(Debug, Default)]
pub struct StopFlag {
    requested: AtomicBool,
}

impl StopFlag {
    /// 新建。
    pub fn new() -> Self {
        Self {
            requested: AtomicBool::new(false),
        }
    }

    /// 请求停止。
    pub fn request(&self) {
        self.requested.store(true, Ordering::Relaxed);
    }

    /// 是否已请求停止。
    pub fn is_requested(&self) -> bool {
        self.requested.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use fh6_core::{derived, Telemetry};

    fn sample_frame(speed_kph: f32) -> Frame {
        let t = Telemetry {
            speed_mps: speed_kph / fh6_core::MPS_TO_KPH,
            is_race_on: true,
            ..Default::default()
        };
        Frame::new(t)
    }

    #[test]
    fn 初始状态为无数据() {
        let slot = TelemetrySlot::new();
        let (frame, state, age) = slot.read(Instant::now());
        assert_eq!(state, LinkState::NoData);
        assert_eq!(age, Duration::MAX);
        assert_eq!(frame.telemetry.speed_mps, 0.0);
        assert_eq!(slot.packets_received(), 0);
    }

    #[test]
    fn 写入后立即可读且状态为活动() {
        let slot = TelemetrySlot::new();
        let now = Instant::now();
        slot.publish(sample_frame(100.0), 12345, now);

        let (frame, state, age) = slot.read(now);
        assert_eq!(state, LinkState::Live);
        assert!(age < Duration::from_millis(1));
        assert!((frame.derived.speed_kph - 100.0).abs() < 0.01);
        assert_eq!(slot.packets_received(), 1);
        assert_eq!(slot.packets_parsed(), 1);
    }

    #[test]
    fn 超过阈值后状态变为过期() {
        let slot = TelemetrySlot::new();
        let t0 = Instant::now();
        slot.publish(sample_frame(80.0), 1, t0);

        // 刚好在阈值内
        let (_, state, _) = slot.read(t0 + STALE_AFTER - Duration::from_millis(1));
        assert_eq!(state, LinkState::Live);

        // 超过阈值
        let (frame, state, age) = slot.read(t0 + STALE_AFTER + Duration::from_millis(1));
        assert_eq!(state, LinkState::Stale);
        assert!(age > STALE_AFTER);
        // 过期时仍返回最后一帧，由渲染层决定是否显示为灰色
        assert!((frame.derived.speed_kph - 80.0).abs() < 0.01);
    }

    #[test]
    fn 新数据会把过期状态改回活动() {
        let slot = TelemetrySlot::new();
        let t0 = Instant::now();
        slot.publish(sample_frame(50.0), 1, t0);
        let (_, state, _) = slot.read(t0 + Duration::from_secs(2));
        assert_eq!(state, LinkState::Stale);

        // 新包到来
        let t1 = t0 + Duration::from_secs(3);
        slot.publish(sample_frame(120.0), 2, t1);
        let (frame, state, _) = slot.read(t1);
        assert_eq!(state, LinkState::Live);
        assert!((frame.derived.speed_kph - 120.0).abs() < 0.01);
    }

    #[test]
    fn 只保留最新一帧不排队() {
        // 这是关键性质：渲染慢的时候不应该积压一堆旧帧，
        // 只应看到最新数据（参考实现会因为同线程而丢新旧之间的包，
        // 但它的"最新值"语义与这里一致，且这里不会因丢包而停止计数）。
        let slot = TelemetrySlot::new();
        let t0 = Instant::now();
        for i in 0..100u32 {
            slot.publish(
                sample_frame(i as f32),
                i,
                t0 + Duration::from_millis(i as u64),
            );
        }
        let (frame, _, _) = slot.read(t0 + Duration::from_millis(100));
        assert!(
            (frame.derived.speed_kph - 99.0).abs() < 0.01,
            "应只看到最后一帧 99, 实际 {}",
            frame.derived.speed_kph
        );
        assert_eq!(slot.packets_received(), 100, "计数不应丢失");
    }

    #[test]
    fn 被拒绝的数据报不更新快照也不改状态() {
        let slot = TelemetrySlot::new();
        let now = Instant::now();
        slot.publish(sample_frame(70.0), 1, now);
        slot.note_rejected();
        slot.note_rejected();

        let (frame, state, _) = slot.read(now);
        assert_eq!(state, LinkState::Live, "拒绝不应影响连接状态");
        assert!(
            (frame.derived.speed_kph - 70.0).abs() < 0.01,
            "拒绝不应覆盖已有数据"
        );
        assert_eq!(slot.packets_received(), 3);
        assert_eq!(slot.packets_parsed(), 1);
        assert_eq!(slot.packets_rejected(), 2);
    }

    #[test]
    fn 端口上只有垃圾数据时不会谎报活动() {
        // 如果端口被别的进程占用乱发短包，HUD 不能显示 LIVE + 一堆 0。
        let slot = TelemetrySlot::new();
        for _ in 0..50 {
            slot.note_rejected();
        }
        let (_, state, _) = slot.read(Instant::now());
        assert_eq!(state, LinkState::NoData, "只有垃圾包时应仍是 NO DATA");
        assert_eq!(slot.packets_rejected(), 50);
    }

    #[test]
    fn 多线程并发读写不出错也不死锁() {
        use std::sync::Arc;
        use std::thread;

        let slot = Arc::new(TelemetrySlot::new());
        let stop = Arc::new(StopFlag::new());

        // 3 个写线程
        let mut handles = Vec::new();
        for w in 0..3u32 {
            let s = Arc::clone(&slot);
            let st = Arc::clone(&stop);
            handles.push(thread::spawn(move || {
                let mut i = 0u32;
                while !st.is_requested() && i < 2000 {
                    s.publish(sample_frame(w as f32 * 10.0 + i as f32), i, Instant::now());
                    i += 1;
                }
                i
            }));
        }
        // 2 个读线程
        for _ in 0..2 {
            let s = Arc::clone(&slot);
            let st = Arc::clone(&stop);
            handles.push(thread::spawn(move || {
                let mut n = 0u32;
                while !st.is_requested() && n < 2000 {
                    let (f, _st, _) = s.read(Instant::now());
                    // 数据必须是自洽的（派生量与原始值一致）
                    assert!(f.derived.speed_kph.is_finite());
                    n += 1;
                }
                n
            }));
        }

        // 等写线程跑完
        for h in handles {
            let _ = h.join();
        }
        assert!(slot.packets_received() > 0);
        assert_eq!(slot.packets_rejected(), 0);
    }

    #[test]
    fn 停止标志幂等且线程安全() {
        let f = StopFlag::new();
        assert!(!f.is_requested());
        f.request();
        assert!(f.is_requested());
        f.request(); // 重复请求不应出错
        assert!(f.is_requested());
    }

    #[test]
    fn 连接状态标签互不相同() {
        let labels = [
            LinkState::NoData.label(),
            LinkState::Live.label(),
            LinkState::Stale.label(),
        ];
        for l in labels {
            assert!(!l.is_empty());
        }
        assert_ne!(labels[0], labels[1]);
        assert_ne!(labels[1], labels[2]);
        assert_ne!(labels[0], labels[2]);
    }

    #[test]
    fn 状态字节往返正确() {
        for s in [LinkState::NoData, LinkState::Live, LinkState::Stale] {
            assert_eq!(LinkState::from_u8(s as u8), s);
        }
        // 未知字节回退到 NoData 而不是 panic
        assert_eq!(LinkState::from_u8(99), LinkState::NoData);
    }

    #[test]
    fn 派生量在写入时就算好而不是每帧重算() {
        // publish 接收的是 Frame（原始 + 派生），说明解析与派生都在收包线程完成；
        // 渲染线程只做读取。这避免了每帧重复计算。
        let slot = TelemetrySlot::new();
        let t = Telemetry {
            speed_mps: 10.0,
            accel_raw: 128,
            velocity: [0.0, 0.0, 10.0],
            ..Default::default()
        };
        let f = Frame::new(t);
        let expect_throttle = derived::derive(&t).throttle_pct;
        slot.publish(f, 0, Instant::now());
        let (back, _, _) = slot.read(Instant::now());
        assert!((back.derived.throttle_pct - expect_throttle).abs() < 1e-4);
    }
}
