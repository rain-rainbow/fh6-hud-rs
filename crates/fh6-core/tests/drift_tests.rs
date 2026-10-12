//! 漂移状态机的测试。
//!
//! # 为什么这些测试重要
//!
//! 漂移评分/风险/风格这套逻辑**没有公开规范**，阈值全部是逆向出来的魔法数字。
//! 一旦有人"顺手"改一个系数，手感就会悄悄变掉，而且**只有实车才能发现**。
//!
//! 这些测试把关键行为**钉死**：
//! * 每个标签的边界值（如风险 28/58/84 与 SAFE/EDGE/RISK/MAX 的对应）；
//! * 状态机的转移条件与优先顺序；
//! * 事件的武装/恢复/冷却时序；
//! * 时间步长无关性（同样的总时长、不同的帧率，结果应一致）。
//!
//! 另外，全部不需要启动窗口或连接游戏。
//!
//! 测试函数名用中文描述意图，因此关闭 `non_snake_case` 检查。
#![allow(non_snake_case)]

use fh6_core::drift::*;
use fh6_core::{derived, Telemetry};

/// 构造一帧遥测。
///
/// `speed_kph` / `angle_deg` / `slip` 是最常用的一组控制量：
/// 角度通过速度矢量实现（斜率 = tan(角度)），滑移通过后轴两轮写入。
///
/// 实现 `Copy` 以便在测试里重复使用同一配置而无需 clone。
#[derive(Debug, Clone, Copy)]
struct FrameBuilder {
    speed_kph: f32,
    angle_deg: f32,
    slip: f32,
    throttle: f32,
    brake: f32,
    clutch: f32,
    handbrake: f32,
    steer_pct: f32,
    g_lat: f32,
    g_long: f32,
}

impl Default for FrameBuilder {
    fn default() -> Self {
        Self {
            speed_kph: 0.0,
            angle_deg: 0.0,
            slip: 0.0,
            throttle: 0.0,
            brake: 0.0,
            clutch: 0.0,
            handbrake: 0.0,
            steer_pct: 0.0,
            g_lat: 0.0,
            g_long: 0.0,
        }
    }
}

impl FrameBuilder {
    fn speed(mut self, kph: f32) -> Self {
        self.speed_kph = kph;
        self
    }
    fn angle(mut self, deg: f32) -> Self {
        self.angle_deg = deg;
        self
    }
    fn slip(mut self, s: f32) -> Self {
        self.slip = s;
        self
    }
    fn throttle(mut self, pct: f32) -> Self {
        self.throttle = pct;
        self
    }
    fn brake(mut self, pct: f32) -> Self {
        self.brake = pct;
        self
    }
    fn clutch(mut self, pct: f32) -> Self {
        self.clutch = pct;
        self
    }
    fn handbrake(mut self, pct: f32) -> Self {
        self.handbrake = pct;
        self
    }
    fn steer(mut self, pct: f32) -> Self {
        self.steer_pct = pct;
        self
    }
    fn build(self) -> Telemetry {
        // 角度：以世界 Z 轴为基准，tan(angle) = vx / vz
        let rad = self.angle_deg.to_radians();
        let speed_mps = self.speed_kph / fh6_core::MPS_TO_KPH;
        let vz = speed_mps * rad.cos();
        let vx = speed_mps * rad.sin();

        // 后轴两轮写入同样的滑移（rear_combined_slip 取绝对值的平均）
        let mut wheel_rl = fh6_core::Wheel::default();
        let mut wheel_rr = fh6_core::Wheel::default();
        wheel_rl.combined_slip = self.slip;
        wheel_rr.combined_slip = self.slip;

        Telemetry {
            is_race_on: true,
            speed_mps,
            velocity: [vx, 0.0, vz],
            acceleration: [
                self.g_lat * fh6_core::STANDARD_GRAVITY,
                0.0,
                self.g_long * fh6_core::STANDARD_GRAVITY,
            ],
            accel_raw: (self.throttle / 100.0 * 255.0).round().clamp(0.0, 255.0) as u8,
            brake_raw: (self.brake / 100.0 * 255.0).round().clamp(0.0, 255.0) as u8,
            clutch_raw: (self.clutch / 100.0 * 255.0).round().clamp(0.0, 255.0) as u8,
            handbrake_raw: (self.handbrake / 100.0 * 255.0).round().clamp(0.0, 255.0) as u8,
            steer_raw: (self.steer_pct / 100.0 * 127.0)
                .round()
                .clamp(-127.0, 127.0) as i8,
            engine_max_rpm: 8000.0,
            current_engine_rpm: 5000.0,
            gear_raw: 3,
            wheel_rl,
            wheel_rr,
            ..Default::default()
        }
    }
}

/// 跑 N 帧，返回最后一帧的快照。
fn run(judge: &mut DriftJudge, fb: FrameBuilder, frames: usize, dt: f32) -> DriftSnapshot {
    let t = fb.build();
    let d = derived::derive(&t);
    let mut snap = judge.update(&t, &d, dt);
    for _ in 1..frames {
        snap = judge.update(&t, &d, dt);
    }
    snap
}

const DT60: f32 = 1.0 / 60.0;

// ============================================================================
// 基础状态
// ============================================================================

#[test]
fn 静止时处于抓地状态且无风险() {
    let mut judge = DriftJudge::new();
    let snap = run(&mut judge, FrameBuilder::default(), 60, DT60);
    assert_eq!(snap.drift_state, DriftState::Grip);
    assert_eq!(snap.risk_label, RiskLabel::Safe);
    assert_eq!(snap.style, Style::Grip);
    assert!(!snap.holding_drift);
    assert_eq!(snap.counter_pct, 0.0);
    assert_eq!(snap.flow_pct, 0.0);
}

#[test]
fn 新判定器初始状态为零() {
    let judge = DriftJudge::new();
    assert_eq!(judge.chaos_penalty(), 0.0);
    assert_eq!(judge.counter_deficit(), 0.0);
}

#[test]
fn 重置后状态回到初始() {
    let mut judge = DriftJudge::new();
    run(
        &mut judge,
        FrameBuilder::default()
            .speed(120.0)
            .angle(35.0)
            .slip(1.0)
            .throttle(80.0),
        60,
        DT60,
    );
    judge.reset();
    let snap = run(&mut judge, FrameBuilder::default(), 5, DT60);
    assert_eq!(snap.drift_state, DriftState::Grip);
    assert_eq!(snap.flow_pct, 0.0);
    assert_eq!(snap.spin_risk, 0.0);
    assert_eq!(snap.god_entry_count, 0);
}

// ============================================================================
// 反打量
// ============================================================================

#[test]
fn 反打量需要角度与方向盘同号() {
    // 同号 → 有反打；异号 → 无反打。
    //
    // 稳态反打量的公式是 `|steer_pct| * (0.45 + 0.55 * 角度因子)`，其中
    // 角度因子 = clamp((|angle| - 8) / 42, 0, 1)。所以 30° 时角度因子 ≈ 0.524，
    // 满打（100%）的理论上界约 73.8%，steer=60% 时约 44.2%。
    // 测试值按此公式核算，不要凭直觉写。
    let mut judge = DriftJudge::new();
    let same = run(
        &mut judge,
        FrameBuilder::default()
            .speed(80.0)
            .angle(30.0)
            .slip(0.8)
            .steer(60.0),
        200,
        DT60,
    );
    assert!(same.countering, "同号应为反打");
    // 手算: 59.84 * (0.45 + 0.55 * (30-8)/42) = 44.17
    let expected = 59.84 * (0.45 + 0.55 * (30.0 - COUNTER_MIN_ANGLE) / COUNTER_ANGLE_SPAN);
    assert!(
        (same.counter_pct - expected).abs() < 0.5,
        "反打量应约 {expected:.2}, 实际 {:.2}",
        same.counter_pct
    );

    let mut judge = DriftJudge::new();
    let opposite = run(
        &mut judge,
        FrameBuilder::default()
            .speed(80.0)
            .angle(30.0)
            .slip(0.8)
            .steer(-60.0),
        200,
        DT60,
    );
    assert!(!opposite.countering, "异号不应算反打");
    assert!(opposite.counter_pct < 1.0, "异号反打量应趋近 0");
}

#[test]
fn 反打量随角度增大而增大且符合公式() {
    // 逐个角度核对稳态反打量，确保比例系数没被改坏。
    for (angle, steer) in [
        (10.0f32, 60.0f32),
        (20.0, 60.0),
        (30.0, 60.0),
        (40.0, 60.0),
        (50.0, 60.0),
    ] {
        let mut judge = DriftJudge::new();
        let snap = run(
            &mut judge,
            FrameBuilder::default()
                .speed(80.0)
                .angle(angle)
                .slip(0.8)
                .steer(steer),
            240,
            DT60,
        );
        let angle_factor = ((angle - COUNTER_MIN_ANGLE) / COUNTER_ANGLE_SPAN).clamp(0.0, 1.0);
        let expected = (steer / 100.0 * 127.0 / 127.0 * 100.0)
            * (COUNTER_GAIN_MIN + COUNTER_GAIN_SPAN * angle_factor);
        assert!(
            (snap.counter_pct - expected).abs() < 1.0,
            "角度 {angle}° 时反打量应为 {expected:.2}, 实际 {:.2}",
            snap.counter_pct
        );
    }
}

#[test]
fn 角度不足8度时不产生反打量() {
    let mut judge = DriftJudge::new();
    let snap = run(
        &mut judge,
        FrameBuilder::default()
            .speed(80.0)
            .angle(5.0)
            .slip(0.8)
            .steer(100.0),
        120,
        DT60,
    );
    assert!(!snap.countering);
    assert!(snap.counter_pct < 1.0);
}

// ============================================================================
// 保持计时
// ============================================================================

#[test]
fn 满足条件时累计保持时间而中断足够久清零() {
    let mut judge = DriftJudge::new();
    let drift = FrameBuilder::default()
        .speed(60.0)
        .angle(25.0)
        .slip(0.6)
        .throttle(60.0);

    // 保持 2 秒
    let snap = run(&mut judge, drift, 120, DT60);
    assert!(snap.holding_drift);
    assert!(
        (snap.hold_seconds - 2.0).abs() < 0.1,
        "应约 2 秒, 实际 {}",
        snap.hold_seconds
    );

    // 中断 0.2 秒（不足 0.45 秒防抖阈值）→ 不应清零
    let coast = FrameBuilder::default().speed(60.0).angle(0.0).slip(0.0);
    let snap = run(&mut judge, coast, 12, DT60);
    assert!(
        snap.hold_seconds > 1.5,
        "中断不足防抖阈值不应清零, 实际 {}",
        snap.hold_seconds
    );

    // 再中断 0.6 秒 → 应清零
    let snap = run(&mut judge, coast, 36, DT60);
    assert_eq!(snap.hold_seconds, 0.0, "中断超时后应清零");
}

#[test]
fn 保持不足十四公里每小时不累计() {
    let mut judge = DriftJudge::new();
    let snap = run(
        &mut judge,
        FrameBuilder::default().speed(10.0).angle(40.0).slip(1.0),
        120,
        DT60,
    );
    assert!(!snap.holding_drift, "速度不足不应算保持");
    assert_eq!(snap.hold_seconds, 0.0);
}

// ============================================================================
// FLOW
// ============================================================================

#[test]
fn 稳定漂移的flow远高于来回甩尾() {
    // 稳定的持续漂移应拿到高 FLOW；方向来回甩会丢掉稳定性分与冷静奖励，
    // 且保持时间几乎无法累计，FLOW 会大幅下降。
    let mut stable = DriftJudge::new();
    let snap_stable = run(
        &mut stable,
        FrameBuilder::default()
            .speed(80.0)
            .angle(30.0)
            .slip(0.9)
            .throttle(60.0)
            .steer(20.0),
        300,
        DT60,
    );

    // 每 2 帧翻转方向（相当于疯狂甩尾）
    let mut chaotic = DriftJudge::new();
    let mut snap_chaotic = DriftSnapshot::default();
    for i in 0..300 {
        let flip = if i % 4 < 2 { 35.0 } else { -35.0 };
        let thr = if i % 4 < 2 { 95.0 } else { 20.0 };
        let tel = FrameBuilder::default()
            .speed(80.0)
            .angle(flip)
            .slip(0.9)
            .throttle(thr)
            .build();
        let d = derived::derive(&tel);
        snap_chaotic = chaotic.update(&tel, &d, DT60);
    }

    assert!(
        snap_stable.flow_pct > snap_chaotic.flow_pct + 20.0,
        "稳定漂移 FLOW({:.1}) 应明显高于来回甩尾 FLOW({:.1})",
        snap_stable.flow_pct,
        snap_chaotic.flow_pct
    );
    // 甩尾会不断打断保持计时
    assert!(
        snap_chaotic.hold_seconds < snap_stable.hold_seconds,
        "甩尾的保持时间({:.3}s)应低于稳定漂移({:.3}s)",
        snap_chaotic.hold_seconds,
        snap_stable.hold_seconds
    );
}

#[test]
fn 方向剧变时产生混乱惩罚() {
    // 角度平缓时无惩罚；发生大幅摆动时惩罚应显著上升。
    let mut judge = DriftJudge::new();

    // 先在 +70° 稳定住
    run(
        &mut judge,
        FrameBuilder::default()
            .speed(90.0)
            .angle(70.0)
            .slip(1.0)
            .throttle(50.0),
        180,
        DT60,
    );
    let calm_penalty = judge.chaos_penalty();
    assert!(
        calm_penalty < 1.0,
        "稳定阶段不应有混乱惩罚, 实际 {calm_penalty:.3}"
    );

    // 突然甩到 -70°
    run(
        &mut judge,
        FrameBuilder::default()
            .speed(90.0)
            .angle(-70.0)
            .slip(1.0)
            .throttle(50.0),
        1,
        DT60,
    );
    let spike = judge.chaos_penalty();
    assert!(spike > 5.0, "方向剧变应产生明显混乱惩罚, 实际 {spike:.3}");
}

#[test]
fn flow始终被钳制在0到100之间() {
    // 极端数值不应该让 FLOW 越界
    let mut judge = DriftJudge::new();
    let snap = run(
        &mut judge,
        FrameBuilder::default()
            .speed(300.0)
            .angle(89.0)
            .slip(50.0)
            .throttle(100.0)
            .steer(100.0),
        400,
        DT60,
    );
    assert!(
        (0.0..=100.0).contains(&snap.flow_pct),
        "FLOW={}",
        snap.flow_pct
    );
    assert!(
        (0.0..=100.0).contains(&snap.spin_risk),
        "RISK={}",
        snap.spin_risk
    );
    assert!((0.0..=100.0).contains(&snap.counter_pct));
    assert!((0.0..=100.0).contains(&snap.flow_stability_pct));
}

// ============================================================================
// SPIN RISK 标签边界
// ============================================================================

#[test]
fn 风险标签阈值边界正确() {
    // 直接验证阈值常量与标签的对应关系（这是最容易被"顺手改坏"的地方）
    assert_eq!(RISK_LABEL_EDGE, 28.0);
    assert_eq!(RISK_LABEL_RISK, 58.0);
    assert_eq!(RISK_LABEL_MAX, 84.0);

    // 通过实际运行验证：大角度+大滑移+大油门应达到高风险
    let mut judge = DriftJudge::new();
    let hot = run(
        &mut judge,
        FrameBuilder::default()
            .speed(90.0)
            .angle(60.0)
            .slip(1.8)
            .throttle(95.0)
            .steer(-10.0),
        400,
        DT60,
    );
    assert!(
        matches!(hot.risk_label, RiskLabel::Risk | RiskLabel::Max),
        "极端输入应达到 RISK 或 MAX, 实际 {:?} (risk={})",
        hot.risk_label,
        hot.spin_risk
    );

    // 温和漂移应处于低风险
    let mut judge2 = DriftJudge::new();
    let calm = run(
        &mut judge2,
        FrameBuilder::default()
            .speed(50.0)
            .angle(15.0)
            .slip(0.35)
            .throttle(30.0)
            .steer(12.0),
        400,
        DT60,
    );
    assert!(
        matches!(calm.risk_label, RiskLabel::Safe | RiskLabel::Edge),
        "温和漂移不应高风险, 实际 {:?} (risk={})",
        calm.risk_label,
        calm.spin_risk
    );
}

#[test]
fn 极低速小角度时风险被缩放() {
    // 速度 <8 且角度 <16 时 risk *= 0.35
    let mut judge = DriftJudge::new();
    let snap = run(
        &mut judge,
        FrameBuilder::default()
            .speed(5.0)
            .angle(10.0)
            .slip(0.1)
            .throttle(10.0),
        200,
        DT60,
    );
    assert!(
        snap.spin_risk < 5.0,
        "极低速小角度风险应很低, 实际 {}",
        snap.spin_risk
    );
}

// ============================================================================
// 漂移状态机
// ============================================================================

#[test]
fn 大角度稳定漂移进入angle状态() {
    let mut judge = DriftJudge::new();
    let snap = run(
        &mut judge,
        FrameBuilder::default()
            .speed(80.0)
            .angle(30.0)
            .slip(0.6)
            .throttle(60.0),
        200,
        DT60,
    );
    assert_eq!(snap.drift_state, DriftState::Angle);
    assert_eq!(snap.state_reason, "ANGLE HOLD");
}

#[test]
fn 后轴大量打滑加高油门进入smoke状态() {
    let mut judge = DriftJudge::new();
    let snap = run(
        &mut judge,
        FrameBuilder::default()
            .speed(80.0)
            .angle(20.0)
            .slip(1.5) // >= 1.15
            .throttle(70.0), // >= 28
        200,
        DT60,
    );
    assert_eq!(snap.drift_state, DriftState::Smoke);
    assert_eq!(snap.state_reason, "REAR SLIP");
}

#[test]
fn 状态优先级smoke高于angle() {
    // 同时满足 SMOKE 与 ANGLE 条件时，SMOKE 优先
    let mut judge = DriftJudge::new();
    let snap = run(
        &mut judge,
        FrameBuilder::default()
            .speed(80.0)
            .angle(40.0)
            .slip(1.5)
            .throttle(70.0),
        200,
        DT60,
    );
    assert_eq!(snap.drift_state, DriftState::Smoke, "SMOKE 应优先于 ANGLE");
}

#[test]
fn 速度不足八且角度不足五时回到抓地() {
    let mut judge = DriftJudge::new();
    // 先建立漂移
    run(
        &mut judge,
        FrameBuilder::default()
            .speed(80.0)
            .angle(30.0)
            .slip(1.0)
            .throttle(70.0),
        100,
        DT60,
    );
    // 再停车
    let snap = run(
        &mut judge,
        FrameBuilder::default().speed(3.0).angle(2.0),
        30,
        DT60,
    );
    assert_eq!(snap.drift_state, DriftState::Grip);
    assert_eq!(snap.style, Style::Grip);
}

#[test]
fn 所有状态标签都有对应的原因文本() {
    for st in [
        DriftState::Grip,
        DriftState::Entry,
        DriftState::Hold,
        DriftState::Angle,
        DriftState::Smoke,
        DriftState::Spin,
    ] {
        assert!(!st.label().is_empty());
        assert!(!st.reason().is_empty());
    }
}

// ============================================================================
// 弹窗
// ============================================================================

#[test]
fn 弹窗优先级数值与参考实现一致() {
    assert_eq!(Popup::SpinSave.priority(), 100);
    assert_eq!(Popup::BigAngle.priority(), 95);
    assert_eq!(Popup::LongHold.priority(), 80);
    assert_eq!(Popup::FlowLock.priority(), 75);
    assert_eq!(Popup::DeepLine.priority(), 60);
    assert_eq!(Popup::LimitEdge.priority(), 55);
    assert_eq!(Popup::SmokeRun.priority(), 45);
    assert_eq!(Popup::Entry.priority(), 40);

    // hero 级：优先级 >= 75
    assert!(Popup::SpinSave.is_hero());
    assert!(Popup::BigAngle.is_hero());
    assert!(Popup::LongHold.is_hero());
    assert!(Popup::FlowLock.is_hero());
    assert!(!Popup::DeepLine.is_hero());
    assert!(!Popup::Entry.is_hero());
}

#[test]
fn 弹窗会随时间长自然消失() {
    let mut judge = DriftJudge::new();
    // 制造 BIG ANGLE：角度从 20° 平滑升到 50°（平滑系数 0.22，需要十几帧）
    run(
        &mut judge,
        FrameBuilder::default().speed(80.0).angle(20.0).slip(0.5),
        30,
        DT60,
    );
    let snap = run(
        &mut judge,
        FrameBuilder::default().speed(80.0).angle(50.0).slip(0.5),
        15,
        DT60,
    );
    assert_eq!(snap.popup.popup, Some(Popup::BigAngle), "应触发 BIG ANGLE");

    // 等 2 秒（BIG ANGLE 默认 1.85 秒 + 冷却 1.2 秒）
    let snap = run(
        &mut judge,
        FrameBuilder::default().speed(80.0).angle(50.0).slip(0.5),
        180,
        DT60,
    );
    assert_ne!(
        snap.popup.popup,
        Some(Popup::BigAngle),
        "BIG ANGLE 应在 2 秒后消失而不是被无限续期"
    );
}

#[test]
fn 同一风格持续期间弹窗不会被重复触发() {
    // 这是开发中实际修掉的缺陷：没有「上一帧状态不同」守卫时，
    // 只要风格持续为 LOCKED，就会每帧请求一次弹窗，导致永不消失。
    let mut judge = DriftJudge::new();
    let fb = FrameBuilder::default()
        .speed(80.0)
        .angle(30.0)
        .slip(0.7)
        .throttle(60.0)
        .steer(30.0);

    // 跑 6 秒
    let mut last = DriftSnapshot::default();
    for _ in 0..360 {
        let t = fb.build();
        let d = derived::derive(&t);
        last = judge.update(&t, &d, DT60);
    }

    // 6 秒后不应仍然显示 1.1–1.8 秒级的短弹窗
    if let Some(p) = last.popup.popup {
        assert!(
            last.popup.ttl <= p.default_seconds(),
            "弹窗剩余时间不应超过其默认时长（说明被反复续期）: {:?} ttl={}",
            p,
            last.popup.ttl
        );
    }
}

#[test]
fn 脚刹操作提示会被触发() {
    let mut judge = DriftJudge::new();
    // 先建立速度与「上一帧刹车很低」的状态
    run(
        &mut judge,
        FrameBuilder::default().speed(60.0).throttle(30.0),
        10,
        DT60,
    );
    // 脚刹突增：brake >= 48 且上一帧 < 20，同时手刹 < 18、速度 > 10
    let snap = run(
        &mut judge,
        FrameBuilder::default()
            .speed(60.0)
            .throttle(30.0)
            .brake(60.0),
        1,
        DT60,
    );
    assert_eq!(
        snap.popup.operation,
        Some(OperationPopup::Brake),
        "脚刹突增应触发 BRAKE 提示"
    );
}

#[test]
fn 离合操作提示会被触发() {
    let mut judge = DriftJudge::new();
    run(
        &mut judge,
        FrameBuilder::default().speed(60.0).throttle(40.0),
        10,
        DT60,
    );
    // 离合突增：clutch >= 58 且上一帧 < 22，油门 >= 18、速度 > 8
    let snap = run(
        &mut judge,
        FrameBuilder::default()
            .speed(60.0)
            .throttle(40.0)
            .clutch(70.0),
        1,
        DT60,
    );
    assert_eq!(
        snap.popup.operation,
        Some(OperationPopup::Clutch),
        "离合突增应触发 CLUTCH 提示"
    );
}

#[test]
fn 手刹操作提示会被触发() {
    let mut judge = DriftJudge::new();
    // 先建立一些速度与前一帧状态
    run(
        &mut judge,
        FrameBuilder::default().speed(60.0).throttle(30.0),
        10,
        DT60,
    );
    // 手刹突增（>=38 且上一帧 <18）
    let snap = run(
        &mut judge,
        FrameBuilder::default()
            .speed(60.0)
            .throttle(30.0)
            .handbrake(60.0),
        1,
        DT60,
    );
    assert_eq!(snap.popup.operation, Some(OperationPopup::Handbrake));
    assert_eq!(OperationPopup::Handbrake.label(), "HANDBRAKE");
}

#[test]
fn 操作提示标签正确() {
    assert_eq!(OperationPopup::Handbrake.label(), "HANDBRAKE");
    assert_eq!(OperationPopup::Clutch.label(), "CLUTCH");
    assert_eq!(OperationPopup::Brake.label(), "BRAKE");
    assert_eq!(OPERATION_POPUP_SECONDS, 0.92);
    assert_eq!(OPERATION_POPUP_COOLDOWN, 0.72);
}

// ============================================================================
// 彩蛋
// ============================================================================

#[test]
fn 第十次进入极限角度区显示cat() {
    let mut judge = DriftJudge::new();

    // 注意：判定用的是**平滑后**的显示角度（参考实现亦然）。
    // 平滑系数 0.22，从 0° 追到 70° 需要十几帧才越过 60° 阈值，
    // 因此每轮要跑够帧数；退出阶段同理，需要足够帧数降回 60° 以下。
    const ENTER_FRAMES: usize = 20;
    const EXIT_FRAMES: usize = 40;

    // 前 9 次进入 >60° 区，应显示 GOD
    for i in 0..9 {
        let snap = run(
            &mut judge,
            FrameBuilder::default()
                .speed(90.0)
                .angle(70.0)
                .slip(1.5)
                .throttle(80.0),
            ENTER_FRAMES,
            DT60,
        );
        assert_eq!(snap.god_entry_count, i + 1, "第 {} 次进入", i + 1);
        assert_eq!(snap.god_word, "GOD");

        // 退出该区域（角度低于 60）以便下次重新计数
        run(
            &mut judge,
            FrameBuilder::default().speed(40.0).angle(10.0),
            EXIT_FRAMES,
            DT60,
        );
    }

    // 第 10 次进入 → CAT
    let snap = run(
        &mut judge,
        FrameBuilder::default()
            .speed(90.0)
            .angle(70.0)
            .slip(1.5)
            .throttle(80.0),
        ENTER_FRAMES,
        DT60,
    );
    assert_eq!(snap.god_entry_count, 10);
    assert_eq!(snap.god_word, "CAT", "第 10 次应切换为 CAT");
}

#[test]
fn 持续停留在极限区只计一次() {
    let mut judge = DriftJudge::new();
    let snap = run(
        &mut judge,
        FrameBuilder::default()
            .speed(90.0)
            .angle(70.0)
            .slip(1.5)
            .throttle(80.0),
        300,
        DT60,
    );
    assert_eq!(snap.god_entry_count, 1, "持续停留应只计一次");
}

// ============================================================================
// 帧率无关性
// ============================================================================

#[test]
fn 同样总时长不同帧率结果应接近() {
    // 参考实现把 dt 硬编码为 0.016，掉帧时计时会漂移。
    // 本实现用真实 dt，因此不同帧率下累计的保持时间应当接近。
    let fb = FrameBuilder::default()
        .speed(70.0)
        .angle(28.0)
        .slip(0.7)
        .throttle(60.0);

    let mut j60 = DriftJudge::new();
    let t = fb.build();
    let d = derived::derive(&t);
    for _ in 0..120 {
        j60.update(&t, &d, 1.0 / 60.0); // 2 秒 @60fps
    }

    let mut j30 = DriftJudge::new();
    for _ in 0..60 {
        j30.update(&t, &d, 1.0 / 30.0); // 2 秒 @30fps
    }

    let snap60 = j60.update(&t, &d, 1.0 / 60.0);
    let snap30 = j30.update(&t, &d, 1.0 / 30.0);

    assert!(
        (snap60.hold_seconds - snap30.hold_seconds).abs() < 0.15,
        "不同帧率的保持计时应接近: 60fps={:.3}s 30fps={:.3}s",
        snap60.hold_seconds,
        snap30.hold_seconds
    );
}

#[test]
fn 非法dt不会panic也不会污染状态() {
    let mut judge = DriftJudge::new();
    let fb = FrameBuilder::default()
        .speed(70.0)
        .angle(28.0)
        .slip(0.7)
        .throttle(60.0);
    let t = fb.build();
    let d = derived::derive(&t);

    for bad_dt in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        let snap = judge.update(&t, &d, bad_dt);
        assert!(snap.hold_seconds.is_finite(), "dt={bad_dt} 后 hold 应有限");
        assert!(snap.flow_pct.is_finite(), "dt={bad_dt} 后 FLOW 应有限");
        assert!(snap.spin_risk.is_finite(), "dt={bad_dt} 后 RISK 应有限");
        assert!((0.0..=100.0).contains(&snap.spin_risk));
    }
}

// ============================================================================
// SPIN SAVE 时序
// ============================================================================

#[test]
fn 危险时刻会武装spin_save() {
    let mut judge = DriftJudge::new();
    // 武装条件：risk >= 76, |angle| >= 24, speed > 12, slip >= 0.55
    run(
        &mut judge,
        FrameBuilder::default()
            .speed(90.0)
            .angle(55.0)
            .slip(1.6)
            .throttle(90.0)
            .steer(-5.0),
        400,
        DT60,
    );

    assert!(
        judge.spin_save_armed(),
        "危险状态应武装 SPIN SAVE（risk={:.1}）",
        judge.spin_risk()
    );
    assert!(judge.spin_risk() >= SAVE_ARM_RISK, "风险应达到武装阈值");
    assert!(judge.display_angle().abs() >= SAVE_ARM_ANGLE);

    // 停下后应解除武装
    run(
        &mut judge,
        FrameBuilder::default().speed(3.0).angle(2.0),
        60,
        DT60,
    );
    assert!(!judge.spin_save_armed(), "停车后应解除武装");
}

#[test]
fn 温和驾驶不会武装spin_save() {
    let mut judge = DriftJudge::new();
    run(
        &mut judge,
        FrameBuilder::default()
            .speed(50.0)
            .angle(12.0)
            .slip(0.35)
            .throttle(30.0)
            .steer(10.0),
        400,
        DT60,
    );
    assert!(!judge.spin_save_armed(), "温和驾驶不应武装 SPIN SAVE");
}

#[test]
fn 触发spin_save后风格被强制为save() {
    let mut judge = DriftJudge::new();

    // ---- 阶段 1：制造危险，武装 ----
    let danger = FrameBuilder::default()
        .speed(90.0)
        .angle(58.0)
        .slip(1.8)
        .throttle(95.0)
        .steer(0.0);
    run(&mut judge, danger, 400, DT60);

    // ---- 阶段 2：恢复到安全区间，且满足 8<=angle<=36 /
    //              risk<=42 / speed>12 / slip>=0.25 ----
    let recover = FrameBuilder::default()
        .speed(60.0)
        .angle(20.0)
        .slip(0.5)
        .throttle(25.0)
        .steer(40.0);
    let mut triggered = false;
    let mut last = DriftSnapshot::default();
    for _ in 0..600 {
        let t = recover.build();
        let d = derived::derive(&t);
        last = judge.update(&t, &d, DT60);
        if last.style == Style::Save {
            triggered = true;
            break;
        }
    }

    assert!(
        triggered,
        "从危险恢复到安全区间应触发 SPIN SAVE（style=SAVE），最后 risk={:.1} state={:?}",
        last.spin_risk, last.drift_state
    );
}

#[test]
fn spin_save有冷却期内的第二次恢复不会触发() {
    // SPIN SAVE 的时序：先在危险状态武装 → 恢复 → 连续 3 帧满足恢复条件才触发
    // → 之后进入 4.2 秒冷却。冷却期内即使再次武装并恢复，也不应触发。
    //
    // 注意：不能用 `style == Save` 判断「是否再次触发」，因为触发后
    // style_override 会把风格锁成 SAVE 长达 2.3 秒，第二次检测会命中这个残留。
    // 必须检测弹窗本身。
    let mut judge = DriftJudge::new();

    let danger = FrameBuilder::default()
        .speed(90.0)
        .angle(58.0)
        .slip(1.8)
        .throttle(95.0)
        .steer(0.0);
    let recover = FrameBuilder::default()
        .speed(60.0)
        .angle(20.0)
        .slip(0.5)
        .throttle(25.0)
        .steer(40.0);

    /// 跑恢复阶段，返回 SPIN SAVE 弹窗**首次出现**时相对本次调用的帧号。
    ///
    /// ⚠️ 必须先等上一轮残留的 SPIN SAVE 弹窗消失再开始计数，否则会把
    /// 上一次的弹窗当成新触发（本测试初版就踩了这个坑：弹窗 TTL 是 1.95 秒，
    /// 而重新武装只要 0.37 秒，残留必然存在）。
    fn recover_until_save(
        judge: &mut DriftJudge,
        fb: FrameBuilder,
        frames: usize,
    ) -> Option<usize> {
        let mut waiting_for_clear = true;
        for i in 0..frames {
            let t = fb.build();
            let d = derived::derive(&t);
            let s = judge.update(&t, &d, DT60);
            if waiting_for_clear {
                if s.popup.popup != Some(Popup::SpinSave) {
                    waiting_for_clear = false;
                }
                continue;
            }
            if s.popup.popup == Some(Popup::SpinSave) {
                return Some(i);
            }
        }
        None
    }

    // ---- 阶段 1：武装后恢复，记录触发时刻 ----
    run(&mut judge, danger, 400, DT60);

    // ⚠️ 这里必须限制帧数，否则会把「冷却窗口」也一起跑过去，
    // 导致后面的断言其实是在冷却之外做的（本测试初版就犯了这个错）。
    // SPIN SAVE 需要连续 3 帧满足恢复条件，第 1 帧最早就可能出现，
    // 给它 60 帧（1 秒）余量足够。
    let first = recover_until_save(&mut judge, recover, 60);
    let first_frame = first.expect("第一次应能触发 SPIN SAVE");
    // 第一次触发时已经过去了这么多秒
    let elapsed = first_frame as f32 * DT60;
    assert!(
        elapsed < SAVE_COOLDOWN_SECONDS,
        "第一次触发应在冷却时长的早期完成, 实际 {elapsed:.2}s"
    );

    // ---- 阶段 2：仍在冷却窗口内，重新武装并恢复 → 不应触发 ----
    // 预算：SAVE_COOLDOWN_SECONDS - elapsed，留 0.4 秒余量
    let budget_frames = (((SAVE_COOLDOWN_SECONDS - elapsed - 0.4) / DT60) as usize).max(1);
    // 前半段用于重新武装（危险状态），后半段用于恢复尝试
    let arm_frames = budget_frames / 3;
    let try_frames = budget_frames - arm_frames;
    run(&mut judge, danger, arm_frames, DT60);
    let second = recover_until_save(&mut judge, recover, try_frames);
    // 冷却是否真的拦住了第二次触发，用一个额外的性质来确认：
    // 此时「恢复条件」应该已经被充分满足（recover_frames 远超阈值），
    // 唯一的拦截原因只可能是冷却。这比单纯断言 None 更有说服力。
    assert!(
        judge.spin_save_recover_frames() >= SAVE_RECOVER_FRAMES,
        "恢复条件本应已被满足（帧数 {}），否则这个测试没在检验冷却",
        judge.spin_save_recover_frames()
    );
    assert!(
        second.is_none(),
        "冷却期内的第二次恢复不应触发 SPIN SAVE（预算 {budget_frames} 帧, 已过 {elapsed:.2}s）"
    );

    // ---- 阶段 3：等冷却彻底走完，再武装 + 恢复 → 应能触发 ----
    run(&mut judge, recover, 300, DT60); // 5 秒，确保冷却归零
    run(&mut judge, danger, 80, DT60); // 重新武装
    let third = recover_until_save(&mut judge, recover, 600);
    assert!(third.is_some(), "冷却结束后应能再次触发 SPIN SAVE");
}

// ============================================================================
// 快照与枚举
// ============================================================================

#[test]
fn 快照是Copy类型以便渲染层零成本传递() {
    fn assert_copy<T: Copy>() {}
    assert_copy::<DriftSnapshot>();
    assert_copy::<PopupState>();
}

#[test]
fn 所有枚举标签非空且互不相同() {
    let states = [
        DriftState::Grip,
        DriftState::Entry,
        DriftState::Hold,
        DriftState::Angle,
        DriftState::Smoke,
        DriftState::Spin,
    ];
    let labels: Vec<&str> = states.iter().map(|s| s.label()).collect();
    for l in &labels {
        assert!(!l.is_empty());
    }
    for i in 0..labels.len() {
        for j in (i + 1)..labels.len() {
            assert_ne!(labels[i], labels[j], "状态标签不应重复");
        }
    }

    let styles = [
        Style::Grip,
        Style::Entry,
        Style::Clean,
        Style::Smooth,
        Style::Deep,
        Style::Locked,
        Style::Wild,
        Style::Edge,
        Style::Danger,
        Style::Save,
        Style::Kick,
        Style::Brake,
    ];
    for s in styles {
        assert!(!s.label().is_empty());
    }

    let popups = [
        Popup::SpinSave,
        Popup::BigAngle,
        Popup::LongHold,
        Popup::FlowLock,
        Popup::DeepLine,
        Popup::LimitEdge,
        Popup::SmokeRun,
        Popup::Entry,
    ];
    for p in popups {
        assert!(!p.label().is_empty());
        assert!(p.default_seconds() >= 0.25, "时长不应过短");
    }
}

#[test]
fn 快照角度与G值经过平滑不会跳变() {
    let mut judge = DriftJudge::new();
    // 从 0 度瞬间跳到 80 度
    let snap1 = run(
        &mut judge,
        FrameBuilder::default().speed(80.0).angle(80.0).slip(1.0),
        1,
        DT60,
    );
    // 第一帧应只走 22% 的平滑量（0 + (80-0)*0.22 ≈ 17.6）
    assert!(
        snap1.display_angle > 0.0 && snap1.display_angle < 30.0,
        "首帧不应直接跳到 80 度, 实际 {}",
        snap1.display_angle
    );

    // 持续下去应接近 80
    let snap2 = run(
        &mut judge,
        FrameBuilder::default().speed(80.0).angle(80.0).slip(1.0),
        120,
        DT60,
    );
    assert!(
        (snap2.display_angle - 80.0).abs() < 2.0,
        "稳定后应接近 80 度, 实际 {}",
        snap2.display_angle
    );
}
