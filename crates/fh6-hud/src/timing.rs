//! 帧率控制与活动检测。
//!
//! 这一层负责回答两个问题：
//!
//! 1. **这一帧该不该现在画？** —— 静止时降到低频，有变化时跑满帧率；
//! 2. **画面有没有变化？** —— 有变化才重绘（配合 [`crate::gauge::DirtyRegion`]）。
//!
//! 参考实现没有这两个概念：它每 16 ms 无条件重绘整块屏幕，哪怕车停在车库里、
//! 哪怕游戏已经关了。那是它最大的资源问题。这里把它们变成显式的、可测试的逻辑。
//!
//! # 为什么用「数值变化」而不是「脏区非空」判断活动
//!
//! 脏区是渲染层根据"哪些组件需要重画"登记的，可能因为动画（例如弹窗淡出）
//! 而为空却仍在变化。而"活动"应该反映**是否有真实数据在动**，
//! 因此这里比较的是遥测数值本身。

use std::time::{Duration, Instant};

use fh6_core::Frame;

/// 判定「数值没变」的容差。
///
/// 浮点数值几乎不可能逐位相等，所以必须用容差。取值需要满足：
/// * 足够大：抑制末位抖动造成的无意义重绘；
/// * 足够小：车速变化 0.01 km/h、转速变化 1 rpm 这类真实变化仍能触发重绘。
pub const EPSILON: f32 = 0.01;

/// 会被画到屏幕上的那组数值，用于「这一帧和上一帧看起来是不是一样」的比较。
///
/// # 为什么要单独抽一个结构
///
/// 直接比较两个 [`Frame`] 有两个问题：
/// 1. `Frame` 里的 `Telemetry` 有几十个字段，其中大部分（角速度、
///    悬挂行程、轮胎温度…）当前根本不显示，比较它们会让"数值没变"
///    这个判断过度敏感 —— 车在坡道上轻微晃动就持续触发重绘；
/// 2. 比较全部字段要拷贝整个结构。
///
/// 因此这里只提取**确实会显示**的量。新增一个显示项时，
/// 记得往 [`VisualKey::of`] 里加一行，否则该项变化不会触发重绘。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisualKey {
    speed_kph: f32,
    drift_angle_deg: f32,
    g_lateral: f32,
    g_longitudinal: f32,
    g_vertical: f32,
    rear_slip: f32,
    throttle_pct: f32,
    brake_pct: f32,
    clutch_pct: f32,
    handbrake_pct: f32,
    steer_pct: f32,
    rpm_ratio: f32,
    current_lap_s: f32,
    current_race_time_s: f32,
    gear_raw: u8,
    is_race_on: bool,
}

impl VisualKey {
    /// 从一帧中提取可见数值。
    pub fn of(f: &Frame) -> Self {
        let d = &f.derived;
        Self {
            speed_kph: d.speed_kph,
            drift_angle_deg: d.drift_angle_deg,
            g_lateral: d.g_lateral,
            g_longitudinal: d.g_longitudinal,
            g_vertical: d.g_vertical,
            rear_slip: d.rear_slip,
            throttle_pct: d.throttle_pct,
            brake_pct: d.brake_pct,
            clutch_pct: d.clutch_pct,
            handbrake_pct: d.handbrake_pct,
            steer_pct: d.steer_pct,
            rpm_ratio: d.rpm_ratio,
            current_lap_s: f.telemetry.current_lap_s,
            current_race_time_s: f.telemetry.current_race_time_s,
            gear_raw: d.gear_raw,
            is_race_on: f.telemetry.is_race_on,
        }
    }

    /// 与另一个快照相比，是否存在**超过容差的可见变化**。
    ///
    /// 整数语义字段（挡位、比赛状态）用精确不等；浮点字段用 [`EPSILON`]。
    pub fn differs_from(&self, other: &Self) -> bool {
        if self.gear_raw != other.gear_raw || self.is_race_on != other.is_race_on {
            return true;
        }
        let pairs: [(f32, f32); 14] = [
            (self.speed_kph, other.speed_kph),
            (self.drift_angle_deg, other.drift_angle_deg),
            (self.g_lateral, other.g_lateral),
            (self.g_longitudinal, other.g_longitudinal),
            (self.g_vertical, other.g_vertical),
            (self.rear_slip, other.rear_slip),
            (self.throttle_pct, other.throttle_pct),
            (self.brake_pct, other.brake_pct),
            (self.clutch_pct, other.clutch_pct),
            (self.handbrake_pct, other.handbrake_pct),
            (self.steer_pct, other.steer_pct),
            (self.rpm_ratio, other.rpm_ratio),
            (self.current_lap_s, other.current_lap_s),
            (self.current_race_time_s, other.current_race_time_s),
        ];
        pairs.iter().any(|(a, b)| !approximately_equal(*a, *b))
    }
}

/// 比较两帧的"可见数值"是否有实质变化（便捷函数）。
pub fn frame_is_visually_different(a: &Frame, b: &Frame) -> bool {
    VisualKey::of(a).differs_from(&VisualKey::of(b))
}

/// 带 NaN 处理的近似相等。
fn approximately_equal(a: f32, b: f32) -> bool {
    match (a.is_finite(), b.is_finite()) {
        (false, false) => true, // 都是 NaN/Inf → 视为相同，避免持续重绘
        (true, true) => (a - b).abs() <= EPSILON,
        _ => false,
    }
}

/// 帧率控制器。
///
/// 用法：每轮循环调用 [`FrameClock::tick`]，它告诉你要不要渲染、
/// 以及是否应显示为"静止"。
#[derive(Debug, Clone)]
pub struct FrameClock {
    active_interval: Duration,
    idle_interval: Duration,
    idle_throttle: bool,
    last_render: Instant,
    last_change: Instant,
    last_frame: Option<Frame>,
    /// 进入"静止"状态需要持续多久没有变化。
    idle_after: Duration,
}

impl FrameClock {
    /// 由目标帧率构造。
    ///
    /// * `active_fps` — 有变化时的帧率上限；
    /// * `idle_fps` — 静止时的帧率上限；
    /// * `idle_throttle` — 是否启用静止降频。
    pub fn new(active_fps: f32, idle_fps: f32, idle_throttle: bool) -> Self {
        let now = Instant::now();
        let active_interval = interval_from_fps(active_fps);
        let idle_interval = interval_from_fps(idle_fps);
        Self {
            active_interval,
            idle_interval,
            idle_throttle,
            // 把「上次渲染」推到过去一个帧间隔，这样**首次 should_render 立即返回 true**。
            // 若从 now 开始计，HUD 启动后要白等一个帧间隔才出第一帧画面，
            // 在静止降频到 4 fps 的情况下那就是 250 ms 的黑屏。
            last_render: now - idle_interval.max(active_interval),
            last_change: now,
            // 初始就认为"静止"，这样启动后不会立刻满帧空转
            last_frame: None,
            idle_after: Duration::from_millis(400),
        }
    }

    /// 设置「多久没变化算静止」。
    pub fn set_idle_after(&mut self, d: Duration) {
        self.idle_after = d;
    }

    /// 通知时钟：收到了一帧新数据。
    ///
    /// 返回 `true` 表示这一帧的**可见数值**与上次不同（应触发重绘）。
    pub fn observe(&mut self, frame: &Frame) -> bool {
        let changed = match &self.last_frame {
            None => true,
            Some(prev) => frame_is_visually_different(prev, frame),
        };
        self.last_frame = Some(*frame);
        if changed {
            self.last_change = Instant::now();
        }
        changed
    }

    /// 当前是否处于「静止」状态。
    pub fn is_idle(&self) -> bool {
        self.idle_throttle && self.last_change.elapsed() >= self.idle_after
    }

    /// 当前生效的帧间隔。
    pub fn interval(&self) -> Duration {
        if self.is_idle() {
            self.idle_interval
        } else {
            self.active_interval
        }
    }

    /// 该不该现在渲染？返回 `true` 时会把内部计时推进到 `now`。
    ///
    /// 注意：**这个方法只负责节流，不代表"有变化"**。
    /// 是否需要真正重绘由调用方结合脏区判断。
    pub fn should_render(&mut self, now: Instant) -> bool {
        if now.saturating_duration_since(self.last_render) >= self.interval() {
            self.last_render = now;
            true
        } else {
            false
        }
    }

    /// 距离下一次渲染还要等多久（用于 `sleep`）。
    pub fn time_until_next(&self, now: Instant) -> Duration {
        self.interval()
            .saturating_sub(now.saturating_duration_since(self.last_render))
    }

    /// 强制下一轮立即渲染（例如用户切换了主题）。
    pub fn force_next(&mut self) {
        self.last_render = Instant::now() - self.interval();
    }

    /// 距上次数值变化过了多久。
    pub fn since_last_change(&self) -> Duration {
        self.last_change.elapsed()
    }
}

/// 由帧率算帧间隔，并夹到合理范围。
fn interval_from_fps(fps: f32) -> Duration {
    let fps = if fps.is_finite() {
        fps.clamp(0.5, 240.0)
    } else {
        60.0
    };
    Duration::from_secs_f32(1.0 / fps)
}

/// 简单的滑动平均，用于统计实际帧率与耗时。
#[derive(Debug, Clone)]
pub struct MovingAverage {
    window: usize,
    samples: Vec<f32>,
    next: usize,
    sum: f32,
}

impl MovingAverage {
    /// 新建一个窗口大小为 `window` 的平均器。
    pub fn new(window: usize) -> Self {
        let window = window.max(1);
        Self {
            window,
            samples: Vec::with_capacity(window),
            next: 0,
            sum: 0.0,
        }
    }

    /// 加入一个样本。
    pub fn push(&mut self, v: f32) {
        if !v.is_finite() {
            return;
        }
        if self.samples.len() < self.window {
            self.samples.push(v);
            self.sum += v;
        } else {
            self.sum -= self.samples[self.next];
            self.samples[self.next] = v;
            self.sum += v;
        }
        self.next = (self.next + 1) % self.window;
    }

    /// 当前平均值；没有样本时返回 `None`。
    pub fn average(&self) -> Option<f32> {
        if self.samples.is_empty() {
            None
        } else {
            Some(self.sum / self.samples.len() as f32)
        }
    }

    /// 样本数。
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// 是否还没有样本。
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// 清空。
    pub fn clear(&mut self) {
        self.samples.clear();
        self.next = 0;
        self.sum = 0.0;
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use fh6_core::{Telemetry, Wheel};

    fn frame(speed_kph: f32, gear: u8, angle: f32, slip: f32) -> Frame {
        let rad = angle.to_radians();
        let mps = speed_kph / fh6_core::MPS_TO_KPH;
        let mut rl = Wheel::default();
        let mut rr = Wheel::default();
        rl.combined_slip = slip;
        rr.combined_slip = slip;
        Frame::new(Telemetry {
            speed_mps: mps,
            velocity: [mps * rad.sin(), 0.0, mps * rad.cos()],
            gear_raw: gear,
            is_race_on: true,
            wheel_rl: rl,
            wheel_rr: rr,
            ..Default::default()
        })
    }

    #[test]
    fn 完全相同的两帧不算变化() {
        let a = frame(100.0, 3, 20.0, 0.5);
        let b = frame(100.0, 3, 20.0, 0.5);
        assert!(!frame_is_visually_different(&a, &b));
    }

    #[test]
    fn 末位抖动被容差吃掉() {
        let a = frame(100.0, 3, 20.0, 0.5);
        let b = frame(100.0 + EPSILON * 0.5, 3, 20.0, 0.5);
        assert!(
            !frame_is_visually_different(&a, &b),
            "小于容差的变化不应触发重绘"
        );
    }

    #[test]
    fn 超过容差的速度变化算变化() {
        let a = frame(100.0, 3, 20.0, 0.5);
        let b = frame(100.0 + EPSILON * 10.0, 3, 20.0, 0.5);
        assert!(frame_is_visually_different(&a, &b));
    }

    #[test]
    fn 挡位变化算变化() {
        let a = frame(100.0, 3, 20.0, 0.5);
        let b = frame(100.0, 4, 20.0, 0.5);
        assert!(frame_is_visually_different(&a, &b));
    }

    #[test]
    fn 角度与滑移变化都算变化() {
        let base = frame(100.0, 3, 20.0, 0.5);
        assert!(frame_is_visually_different(
            &base,
            &frame(100.0, 3, 30.0, 0.5)
        ));
        assert!(frame_is_visually_different(
            &base,
            &frame(100.0, 3, 20.0, 1.2)
        ));
    }

    #[test]
    fn 比赛状态变化算变化() {
        let a = frame(100.0, 3, 20.0, 0.5);
        let mut b = a;
        b.telemetry.is_race_on = false;
        assert!(frame_is_visually_different(&a, &b));
    }

    #[test]
    fn 两帧都是NaN时视为相同() {
        // 瞬态异常值不应导致每帧重绘
        let mut a = frame(0.0, 0, 0.0, 0.0);
        let mut b = a;
        a.derived.g_lateral = f32::NAN;
        b.derived.g_lateral = f32::NAN;
        assert!(!frame_is_visually_different(&a, &b));

        // 一帧 NaN 一帧正常 → 应算变化（需要重画）
        b.derived.g_lateral = 1.0;
        assert!(frame_is_visually_different(&a, &b));
    }

    #[test]
    fn 初始观察总被认为是变化() {
        let mut c = FrameClock::new(60.0, 4.0, true);
        assert!(c.observe(&frame(0.0, 0, 0.0, 0.0)), "首帧应视为变化");
    }

    #[test]
    fn 静止后帧率降低() {
        let mut c = FrameClock::new(60.0, 4.0, true);
        c.set_idle_after(Duration::from_millis(50));
        c.observe(&frame(100.0, 3, 20.0, 0.5));
        assert!(!c.is_idle(), "刚变化过不应静止");
        assert!(c.interval() < Duration::from_millis(20));

        std::thread::sleep(Duration::from_millis(70));
        assert!(c.is_idle(), "持续无变化后应进入静止");
        assert!(
            c.interval() > Duration::from_millis(200),
            "静止时帧间隔应显著变大, 实际 {:?}",
            c.interval()
        );
    }

    #[test]
    fn 关闭静止节流后帧率不变() {
        let mut c = FrameClock::new(60.0, 1.0, false);
        c.set_idle_after(Duration::from_millis(1));
        c.observe(&frame(10.0, 1, 0.0, 0.0));
        std::thread::sleep(Duration::from_millis(30));
        assert!(!c.is_idle());
        assert!(c.interval() < Duration::from_millis(20));
    }

    #[test]
    fn 变化会把时钟从静止状态唤醒() {
        let mut c = FrameClock::new(60.0, 4.0, true);
        c.set_idle_after(Duration::from_millis(20));
        c.observe(&frame(100.0, 3, 20.0, 0.5));
        std::thread::sleep(Duration::from_millis(40));
        assert!(c.is_idle());

        // 速度变了
        let changed = c.observe(&frame(120.0, 3, 20.0, 0.5));
        assert!(changed);
        assert!(!c.is_idle(), "有变化后应退出静止");
    }

    #[test]
    fn 相同数据不会唤醒静止状态() {
        let mut c = FrameClock::new(60.0, 4.0, true);
        c.set_idle_after(Duration::from_millis(20));
        let f = frame(100.0, 3, 20.0, 0.5);
        c.observe(&f);
        std::thread::sleep(Duration::from_millis(40));
        assert!(c.is_idle());

        let changed = c.observe(&f);
        assert!(!changed, "完全相同的帧不应算变化");
        assert!(c.is_idle(), "应保持静止");
    }

    #[test]
    fn should_render按间隔节流() {
        let mut c = FrameClock::new(60.0, 60.0, false);
        // 用构造后的时刻作基准：构造函数刻意把 last_render 推到过去，
        // 使首帧立即可渲染（见 FrameClock::new 的注释）。
        let t0 = Instant::now();
        assert!(c.should_render(t0), "首次应允许渲染");
        assert!(!c.should_render(t0), "同一时刻不应重复渲染");
        assert!(!c.should_render(t0 + Duration::from_millis(5)));
        assert!(
            c.should_render(t0 + Duration::from_millis(20)),
            "超过帧间隔后应允许"
        );
    }

    #[test]
    fn 首次渲染不需要等待一个帧间隔() {
        // 这是实际运行中会看到的现象：若从 now 开始计时，
        // 静止帧率为 4 fps 时启动后会有约 250 ms 不出画面。
        let c = FrameClock::new(60.0, 4.0, true);
        assert!(
            c.time_until_next(Instant::now()) <= Duration::from_millis(1),
            "启动后应立即可以渲染"
        );
    }

    #[test]
    fn 强制渲染会绕过节流() {
        let mut c = FrameClock::new(1.0, 1.0, false); // 1 fps，间隔 1 秒
        let t0 = Instant::now();
        assert!(c.should_render(t0), "首次应允许渲染");
        assert!(!c.should_render(t0 + Duration::from_millis(10)));
        c.force_next();
        assert!(c.should_render(Instant::now()), "强制后应立即允许渲染");
    }

    #[test]
    fn 帧间隔被夹在合理范围() {
        // 荒谬的帧率不应产生 0 或巨大的间隔
        assert!(interval_from_fps(0.0) <= Duration::from_secs(2));
        assert!(interval_from_fps(-5.0) <= Duration::from_secs(2));
        assert!(interval_from_fps(1e9) >= Duration::from_micros(4000));
        assert!(interval_from_fps(f32::NAN) <= Duration::from_millis(20));
        assert!(interval_from_fps(f32::INFINITY) <= Duration::from_millis(20));
    }

    #[test]
    fn 移动平均窗口与平均值正确() {
        let mut m = MovingAverage::new(3);
        assert!(m.is_empty());
        assert_eq!(m.average(), None);

        m.push(10.0);
        assert_eq!(m.average(), Some(10.0));
        m.push(20.0);
        assert_eq!(m.average(), Some(15.0));
        m.push(30.0);
        assert_eq!(m.average(), Some(20.0));

        // 满窗口后覆盖最旧
        m.push(60.0); // 窗口变成 [60,20,30]
        assert_eq!(m.len(), 3);
        assert!((m.average().unwrap() - (60.0 + 20.0 + 30.0) / 3.0).abs() < 1e-4);

        m.clear();
        assert!(m.is_empty());
    }

    #[test]
    fn 移动平均忽略非有限值() {
        let mut m = MovingAverage::new(4);
        m.push(10.0);
        m.push(f32::NAN);
        m.push(f32::INFINITY);
        m.push(20.0);
        assert_eq!(m.len(), 2, "NaN/Inf 不应进入样本");
        assert_eq!(m.average(), Some(15.0));
    }

    #[test]
    fn 零窗口的移动平均不会panic() {
        let mut m = MovingAverage::new(0);
        m.push(5.0);
        assert_eq!(m.average(), Some(5.0));
        m.push(15.0);
        assert_eq!(m.average(), Some(15.0));
    }

    #[test]
    fn 静止时钟的等待时间不为负() {
        let mut c = FrameClock::new(60.0, 4.0, true);
        let t0 = Instant::now();
        c.should_render(t0);
        // 即使传入"过去"的时刻也不应下溢
        let d = c.time_until_next(t0 - Duration::from_secs(10));
        assert!(d <= Duration::from_secs(1));
    }
}
