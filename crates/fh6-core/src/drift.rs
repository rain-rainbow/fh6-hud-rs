//! 漂移判定状态机：从遥测数据算出 FLOW（流畅度）、SPIN RISK（打滑风险）、
//! 反打量、风格标签、事件弹窗。
//!
//! # 设计原则
//!
//! 1. **纯逻辑**：不碰渲染、不碰 I/O、不读全局状态。给定相同输入序列必然得到
//!    相同输出，因此可以完整单元测试——不需要启动窗口，也不需要连着游戏。
//! 2. **零堆分配**：热路径（每秒 60+ 次）不产生任何分配。弹窗用枚举而非字符串，
//!    轨迹点用固定容量环形缓冲。
//! 3. **与帧率解耦**：所有计时用秒（`f32`）+ 调用方传入的 `dt`，
//!    内部统一换算成帧数。
//!    > 参考实现把 `dt` 硬编码成 `0.016`，掉帧时所有计时都会漂移
//!    > （`LONG HOLD` 的 5 秒会变长、冷却会缩水）。这里改为真实 dt。
//!
//! # 阈值来源与法律说明
//!
//! 本模块的阈值与权重提取自参考实现
//! [O93-3/093_Drift_HUD](https://github.com/O93-3/093_Drift_HUD) 的
//! `_tick()` 方法。该仓库**没有 LICENSE**（GitHub 标记 `NOASSERTION`），
//! 因此这里刻意只提取**功能性参数**（数值阈值、权重、状态转移条件），
//! 这类内容属于功能/事实范畴；**没有**复制或翻译任何创造性表达（渲染代码、
//! 坐标常量、绘制路径）。
//!
//! 每个阈值都标注了来源，便于日后与实车手感对拍调参。
//!
//! # 与参考实现的两处有意偏离
//!
//! 1. `dt` 用真实值而非固定 `0.016`（见上文第 3 点）。
//! 2. 弹窗文本用枚举而非字符串常量池，避免每帧比较字符串。

use crate::telemetry::Telemetry;

// ============================================================================
// 阈值常量
// ============================================================================

/// 显示用漂移角的平滑系数（每帧向目标靠拢的比例）。
pub const ANGLE_SMOOTH: f32 = 0.22;
/// G 值显示的平滑系数。
pub const G_SMOOTH: f32 = 0.18;
/// 反打量的平滑系数。
pub const COUNTER_SMOOTH: f32 = 0.24;
/// FLOW 的平滑系数。
pub const FLOW_SMOOTH: f32 = 0.22;
/// SPIN RISK 的平滑系数（比 FLOW 更灵敏，风险要快点显示出来）。
pub const RISK_SMOOTH: f32 = 0.30;

/// 反打量起效的最小角度（度）。
pub const COUNTER_MIN_ANGLE: f32 = 8.0;
/// 反打量满幅对应的角度跨度（度）。
pub const COUNTER_ANGLE_SPAN: f32 = 42.0;
/// 反打量系数下界与上界（`0.45 + 0.55 * 角度因子`）。
pub const COUNTER_GAIN_MIN: f32 = 0.45;
/// 反打量系数增量。
pub const COUNTER_GAIN_SPAN: f32 = 0.55;

/// 开始累计「保持漂移」的最低速度（km/h）。
pub const HOLD_MIN_SPEED_KPH: f32 = 14.0;
/// 开始累计保持的最低角度（度）。
pub const HOLD_MIN_ANGLE: f32 = 12.0;
/// 开始累计保持的最低后轴滑移。
pub const HOLD_MIN_SLIP: f32 = 0.25;
/// 中断多久才清零保持计时（秒）——防抖。
pub const HOLD_RESET_SECONDS: f32 = 0.45;
/// 保持多久算「长保持」（秒）。
pub const LONG_HOLD_SECONDS: f32 = 5.0;

// ---- FLOW 各项权重 ----
/// 角度项：起始角度与跨度。
pub const FLOW_ANGLE_BASE: f32 = 8.0;
/// 角度项跨度。
pub const FLOW_ANGLE_SPAN: f32 = 42.0;
/// 角度项满权重。
pub const FLOW_ANGLE_MAX: f32 = 36.0;
/// 滑移项：起始滑移。
pub const FLOW_SLIP_BASE: f32 = 0.25;
/// 滑移项跨度。
pub const FLOW_SLIP_SPAN: f32 = 1.45;
/// 滑移项满权重。
pub const FLOW_SLIP_MAX: f32 = 20.0;
/// 油门项：起始油门百分比。
pub const FLOW_THROTTLE_BASE: f32 = 10.0;
/// 油门项跨度。
pub const FLOW_THROTTLE_SPAN: f32 = 55.0;
/// 油门项满权重。
pub const FLOW_THROTTLE_MAX: f32 = 14.0;
/// 反打项满权重。
pub const FLOW_COUNTER_MAX: f32 = 12.0;
/// 稳定性：角度变化率容差（度/帧）。
pub const STAB_ANGLE_TOL: f32 = 7.5;
/// 稳定性：角度稳定性权重。
pub const STAB_ANGLE_MAX: f32 = 9.0;
/// 稳定性：滑移变化率容差。
pub const STAB_SLIP_TOL: f32 = 0.42;
/// 稳定性：滑移稳定性权重。
pub const STAB_SLIP_MAX: f32 = 7.0;
/// 稳定性：油门变化率容差。
pub const STAB_ACCEL_TOL: f32 = 26.0;
/// 稳定性：油门稳定性权重。
pub const STAB_ACCEL_MAX: f32 = 5.0;
/// 稳定性总分（用于换算百分比）。
pub const STAB_TOTAL: f32 = 21.0;
/// 「冷静奖励」起始保持秒数与跨度、满权重。
pub const CALM_BASE_SECONDS: f32 = 1.0;
/// 冷静奖励跨度（秒）。
pub const CALM_SPAN_SECONDS: f32 = 5.0;
/// 冷静奖励满权重。
pub const CALM_MAX: f32 = 8.0;

// ---- 混乱惩罚 ----
/// 角度突变起始阈值（度），超出开始扣分。
pub const CHAOS_ANGLE_BASE: f32 = 8.0;
/// 角度突变跨度。
pub const CHAOS_ANGLE_SPAN: f32 = 16.0;
/// 角度突变最大扣分。
pub const CHAOS_ANGLE_MAX: f32 = 10.0;
/// 滑移突变起始阈值。
pub const CHAOS_SLIP_BASE: f32 = 0.45;
/// 滑移突变跨度。
pub const CHAOS_SLIP_SPAN: f32 = 0.85;
/// 滑移突变最大扣分。
pub const CHAOS_SLIP_MAX: f32 = 10.0;
/// 方向盘突变起始阈值（百分比）。
pub const CHAOS_STEER_BASE: f32 = 22.0;
/// 方向盘突变跨度。
pub const CHAOS_STEER_SPAN: f32 = 55.0;
/// 方向盘突变最大扣分。
pub const CHAOS_STEER_MAX: f32 = 7.0;
/// 低速额外惩罚：速度低于该值时附加。
pub const CHAOS_LOW_SPEED_KPH: f32 = 10.0;
/// 低速额外惩罚值。
pub const CHAOS_LOW_SPEED_PENALTY: f32 = 18.0;
/// 判定「混乱」的惩罚阈值（用于 CHASE 标签）。
pub const CHAOS_CHASE_THRESHOLD: f32 = 14.0;

// ---- FLOW 质量标签阈值 ----
/// LOCKED 的 FLOW 阈值。
pub const FLOW_LOCKED_MIN: f32 = 82.0;
/// LOCKED 要求的稳定性百分比。
pub const FLOW_LOCKED_STABILITY: f32 = 55.0;
/// SMOOTH 的 FLOW 阈值。
pub const FLOW_SMOOTH_MIN: f32 = 66.0;

// ---- SPIN RISK 各项权重 ----
/// 风险角度项：起始角度与跨度。
pub const RISK_ANGLE_BASE: f32 = 14.0;
/// 风险角度项跨度。
pub const RISK_ANGLE_SPAN: f32 = 44.0;
/// 风险角度项满权重。
pub const RISK_ANGLE_MAX: f32 = 50.0;
/// 风险滑移项起始与跨度。
pub const RISK_SLIP_BASE: f32 = 0.45;
/// 风险滑移项跨度。
pub const RISK_SLIP_SPAN: f32 = 1.35;
/// 风险滑移项满权重。
pub const RISK_SLIP_MAX: f32 = 20.0;
/// 风险角度突变项起始与跨度。
pub const RISK_ANGLE_DELTA_BASE: f32 = 3.5;
/// 风险角度突变项跨度。
pub const RISK_ANGLE_DELTA_SPAN: f32 = 14.0;
/// 风险角度突变项满权重。
pub const RISK_ANGLE_DELTA_MAX: f32 = 12.0;
/// 风险滑移突变项起始与跨度。
pub const RISK_SLIP_DELTA_BASE: f32 = 0.18;
/// 风险滑移突变项跨度。
pub const RISK_SLIP_DELTA_SPAN: f32 = 0.75;
/// 风险滑移突变项满权重。
pub const RISK_SLIP_DELTA_MAX: f32 = 10.0;
/// 反打缺口项：起始与跨度。
pub const RISK_DEFICIT_BASE: f32 = 8.0;
/// 反打缺口项跨度。
pub const RISK_DEFICIT_SPAN: f32 = 70.0;
/// 反打缺口项满权重。
pub const RISK_DEFICIT_MAX: f32 = 10.0;
/// 反打需求量起始角度与跨度。
pub const COUNTER_NEED_BASE_ANGLE: f32 = 18.0;
/// 反打需求量角度跨度。
pub const COUNTER_NEED_SPAN: f32 = 42.0;
/// 大油门大角度附加风险的门槛。
pub const RISK_PUSH_ACCEL_MIN: f32 = 62.0;
/// 大油门大角度附加风险要求的最低滑移。
pub const RISK_PUSH_SLIP_MIN: f32 = 0.72;
/// 大油门大角度附加风险要求的最低角度。
pub const RISK_PUSH_ANGLE_MIN: f32 = 22.0;
/// 大油门大角度附加风险满权重。
pub const RISK_PUSH_MAX: f32 = 8.0;
/// 低速大角度附加风险的起始角度。
pub const RISK_LOWSPEED_ANGLE_MIN: f32 = 30.0;
/// 低速大角度附加风险的速度门槛。
pub const RISK_LOWSPEED_KPH: f32 = 18.0;
/// 低速大角度附加风险满权重。
pub const RISK_LOWSPEED_MAX: f32 = 8.0;
/// 稳定漂移的减分权重。
pub const RISK_RELIEF_MAX: f32 = 8.0;
/// 稳定漂移减分的 FLOW 基准与跨度。
pub const RISK_RELIEF_FLOW_BASE: f32 = 58.0;
/// 稳定漂移减分跨度。
pub const RISK_RELIEF_FLOW_SPAN: f32 = 34.0;
/// 极低速小角度时的风险缩放系数。
pub const RISK_LOW_ACTIVITY_SCALE: f32 = 0.35;

// ---- 风险标签阈值 ----
/// MAX 标签阈值。
pub const RISK_LABEL_MAX: f32 = 84.0;
/// RISK 标签阈值。
pub const RISK_LABEL_RISK: f32 = 58.0;
/// EDGE 标签阈值。
pub const RISK_LABEL_EDGE: f32 = 28.0;

// ---- SPIN SAVE 事件 ----
/// 进入「危险」从而武装 SPIN SAVE 的风险阈值。
pub const SAVE_ARM_RISK: f32 = 76.0;
/// 武装所需的最低角度。
pub const SAVE_ARM_ANGLE: f32 = 24.0;
/// 武装所需的最低速度（km/h）。
pub const SAVE_ARM_SPEED_KPH: f32 = 12.0;
/// 武装所需的最低滑移。
pub const SAVE_ARM_SLIP: f32 = 0.55;
/// 触发「已救回」所需的峰值风险。
pub const SAVE_PEAK_RISK: f32 = 78.0;
/// 判定恢复的风险上界。
pub const SAVE_RECOVER_RISK: f32 = 42.0;
/// 判定恢复的角度下界与上界。
pub const SAVE_RECOVER_ANGLE_MIN: f32 = 8.0;
/// 判定恢复的角度上界。
pub const SAVE_RECOVER_ANGLE_MAX: f32 = 36.0;
/// 判定恢复所需的最低滑移。
pub const SAVE_RECOVER_SLIP: f32 = 0.25;
/// 需要连续多少帧满足恢复条件才触发。
pub const SAVE_RECOVER_FRAMES: u32 = 3;
/// 触发后的冷却（秒）。
pub const SAVE_COOLDOWN_SECONDS: f32 = 4.2;
/// 触发后强制显示 SAVE 风格的时长（秒）。
pub const SAVE_STYLE_SECONDS: f32 = 2.3;
/// 低于该速度或角度时解除武装。
pub const SAVE_DISARM_SPEED_KPH: f32 = 8.0;
/// 解除武装的角度阈值。
pub const SAVE_DISARM_ANGLE: f32 = 5.0;

// ---- 漂移状态机 ----
/// `active_hold` 每次判定为漂移时重置到的帧数。
pub const ACTIVE_HOLD_FRAMES: u32 = 10;
/// `entry_hold` 触发时设置的帧数。
pub const ENTRY_HOLD_FRAMES: u32 = 8;
/// 角度型漂移所需的最低角度。
pub const STATE_ANGLE_MIN: f32 = 18.0;
/// 角度型漂移所需的最低速度（km/h）。
pub const STATE_ANGLE_SPEED_KPH: f32 = 14.0;
/// 烟雾型漂移所需的最低滑移。
pub const STATE_SMOKE_SLIP: f32 = 1.15;
/// 烟雾型漂移所需的最低油门百分比。
pub const STATE_SMOKE_ACCEL: f32 = 28.0;
/// 判定为「静止」的速度与角度。
pub const STATE_IDLE_SPEED_KPH: f32 = 8.0;
/// 判定为「静止」的角度。
pub const STATE_IDLE_ANGLE: f32 = 5.0;
/// SPIN 状态要求的最低速度。
pub const STATE_SPIN_SPEED_KPH: f32 = 12.0;

/// 漂移状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DriftState {
    /// 未漂移（默认）。
    #[default]
    Grip,
    /// 刚进入漂移（抑制刷屏）。
    Entry,
    /// 稳定保持。
    Hold,
    /// 大角度保持。
    Angle,
    /// 后轮大量打滑（冒烟）。
    Smoke,
    /// 接近失控。
    Spin,
}

impl DriftState {
    /// 英文显示文本（与参考实现一致，便于对照）。
    pub fn label(self) -> &'static str {
        match self {
            DriftState::Grip => "GRIP",
            DriftState::Entry => "ENTRY",
            DriftState::Hold => "HOLD",
            DriftState::Angle => "ANGLE",
            DriftState::Smoke => "SMOKE",
            DriftState::Spin => "SPIN",
        }
    }

    /// 状态原因文本。
    pub fn reason(self) -> &'static str {
        match self {
            DriftState::Grip => "RESET",
            DriftState::Entry => "INITIATE",
            DriftState::Hold => "STABLE",
            DriftState::Angle => "ANGLE HOLD",
            DriftState::Smoke => "REAR SLIP",
            DriftState::Spin => "LIMIT MAX",
        }
    }
}

/// FLOW 质量标签。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FlowQuality {
    /// 尚未成型。
    #[default]
    Build,
    /// 正在成型的漂移。
    Flow,
    /// 流畅但伴随修正。
    Chase,
    /// 顺滑。
    Smooth,
    /// 锁定（最高）。
    Locked,
}

impl FlowQuality {
    /// 显示文本。
    pub fn label(self) -> &'static str {
        match self {
            FlowQuality::Build => "BUILD",
            FlowQuality::Flow => "FLOW",
            FlowQuality::Chase => "CHASE",
            FlowQuality::Smooth => "SMOOTH",
            FlowQuality::Locked => "LOCKED",
        }
    }
}

/// 打滑风险标签。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RiskLabel {
    /// 安全（默认）。
    #[default]
    Safe,
    /// 接近极限。
    Edge,
    /// 有失控风险。
    Risk,
    /// 极限。
    Max,
}

impl RiskLabel {
    /// 显示文本。
    pub fn label(self) -> &'static str {
        match self {
            RiskLabel::Safe => "SAFE",
            RiskLabel::Edge => "EDGE",
            RiskLabel::Risk => "RISK",
            RiskLabel::Max => "MAX",
        }
    }
}

/// 驾驶风格标签。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Style {
    /// 有抓地力（默认）。
    #[default]
    Grip,
    /// 刚进入。
    Entry,
    /// 干净。
    Clean,
    /// 顺滑。
    Smooth,
    /// 深角度。
    Deep,
    /// 锁定。
    Locked,
    /// 狂野。
    Wild,
    /// 边缘。
    Edge,
    /// 危险。
    Danger,
    /// 救回。
    Save,
    /// 离合踹击（操作弹窗覆盖）。
    Kick,
    /// 制动（操作弹窗覆盖）。
    Brake,
}

impl Style {
    /// 显示文本。
    pub fn label(self) -> &'static str {
        match self {
            Style::Grip => "GRIP",
            Style::Entry => "ENTRY",
            Style::Clean => "CLEAN",
            Style::Smooth => "SMOOTH",
            Style::Deep => "DEEP",
            Style::Locked => "LOCKED",
            Style::Wild => "WILD",
            Style::Edge => "EDGE",
            Style::Danger => "DANGER",
            Style::Save => "SAVE",
            Style::Kick => "KICK",
            Style::Brake => "BRAKE",
        }
    }
}

/// 事件弹窗类型。
///
/// 用枚举而不是字符串：避免每帧分配与字符串比较。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Popup {
    /// 极限救回（最高优先级）。
    SpinSave,
    /// 大角度。
    BigAngle,
    /// 长保持。
    LongHold,
    /// 流畅锁定。
    FlowLock,
    /// 深线。
    DeepLine,
    /// 逼近极限。
    LimitEdge,
    /// 冒烟。
    SmokeRun,
    /// 进入漂移。
    Entry,
}

impl Popup {
    /// 优先级。数值越大越优先，可抢占较低优先级的弹窗。
    pub fn priority(self) -> u8 {
        match self {
            Popup::SpinSave => 100,
            Popup::BigAngle => 95,
            Popup::LongHold => 80,
            Popup::FlowLock => 75,
            Popup::DeepLine => 60,
            Popup::LimitEdge => 55,
            Popup::SmokeRun => 45,
            Popup::Entry => 40,
        }
    }

    /// 默认显示时长（秒）。
    pub fn default_seconds(self) -> f32 {
        match self {
            Popup::SpinSave => 1.95,
            Popup::BigAngle => 1.85,
            Popup::LongHold => 1.65,
            Popup::FlowLock => 1.55,
            Popup::DeepLine => 1.20,
            Popup::LimitEdge => 1.15,
            Popup::SmokeRun => 1.10,
            Popup::Entry => 1.00,
        }
    }

    /// 显示文本。
    pub fn label(self) -> &'static str {
        match self {
            Popup::SpinSave => "SPIN SAVE!",
            Popup::BigAngle => "BIG ANGLE!",
            Popup::LongHold => "LONG HOLD!",
            Popup::FlowLock => "FLOW LOCK!",
            Popup::DeepLine => "DEEP LINE!",
            Popup::LimitEdge => "LIMIT EDGE!",
            Popup::SmokeRun => "SMOKE RUN!",
            Popup::Entry => "ENTRY!",
        }
    }

    /// 是否属于「hero」级弹窗（优先级 ≥ 75），渲染时用更大字号。
    pub fn is_hero(self) -> bool {
        self.priority() >= 75
    }
}

/// 操作提示弹窗（只保留三个，与参考实现 Public Test 02 一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationPopup {
    /// 手刹。
    Handbrake,
    /// 离合。
    Clutch,
    /// 刹车。
    Brake,
}

impl OperationPopup {
    /// 显示文本。
    pub fn label(self) -> &'static str {
        match self {
            OperationPopup::Handbrake => "HANDBRAKE",
            OperationPopup::Clutch => "CLUTCH",
            OperationPopup::Brake => "BRAKE",
        }
    }

    /// 对应的风格覆盖标签。
    fn style_override(self) -> Style {
        match self {
            OperationPopup::Handbrake => Style::Entry,
            OperationPopup::Clutch => Style::Kick,
            OperationPopup::Brake => Style::Brake,
        }
    }
}

/// 操作提示的显示时长（秒）。
pub const OPERATION_POPUP_SECONDS: f32 = 0.92;
/// 操作提示的冷却（秒）。
pub const OPERATION_POPUP_COOLDOWN: f32 = 0.72;
/// 操作提示触发的风格覆盖时长（秒）。
pub const OPERATION_STYLE_SECONDS: f32 = 1.15;

/// 当前弹窗的瞬时状态。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PopupState {
    /// 当前显示的事件弹窗。
    pub popup: Option<Popup>,
    /// 剩余显示时间（秒）。
    pub ttl: f32,
    /// 当前显示的操作提示。
    pub operation: Option<OperationPopup>,
    /// 操作提示剩余显示时间（秒）。
    pub operation_ttl: f32,
    /// 操作提示的总时长（用于画进度条）。
    pub operation_total_ttl: f32,
}

impl PopupState {
    /// 当前弹窗是否处于 hero 级。
    pub fn is_hero(&self) -> bool {
        self.popup.map(|p| p.is_hero()).unwrap_or(false)
    }
}

/// 判定结果快照：一帧的全部输出。
///
/// 全部为 `Copy` 的纯数值，渲染层可以自由按值传递，无分配。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DriftSnapshot {
    /// 平滑后的显示角度。
    pub display_angle: f32,
    /// 平滑后的横向 G。
    pub g_lateral: f32,
    /// 平滑后的纵向 G。
    pub g_longitudinal: f32,
    /// 反打量百分比 0–100。
    pub counter_pct: f32,
    /// 已保持漂移的秒数。
    pub hold_seconds: f32,
    /// FLOW 流畅度 0–100。
    pub flow_pct: f32,
    /// FLOW 的稳定性分项百分比 0–100。
    pub flow_stability_pct: f32,
    /// FLOW 质量标签。
    pub flow_quality: FlowQuality,
    /// SPIN RISK 0–100。
    pub spin_risk: f32,
    /// 风险标签。
    pub risk_label: RiskLabel,
    /// 风格标签。
    pub style: Style,
    /// 漂移状态。
    pub drift_state: DriftState,
    /// 状态原因文本。
    pub state_reason: &'static str,
    /// 是否正在保持漂移。
    pub holding_drift: bool,
    /// 是否正在反打。
    pub countering: bool,
    /// GOD/CAT 彩蛋的当前用词。
    pub god_word: &'static str,
    /// 进入 GOD 区（角度 > 60°）的累计次数。
    pub god_entry_count: u32,
    /// 弹窗状态。
    pub popup: PopupState,
}

/// 漂移判定状态机。
///
/// 每帧调用 [`DriftJudge::update`]，传入遥测与派生量、以及真实的 `dt`（秒）。
///
/// # 用法
///
/// ```
/// use fh6_core::drift::DriftJudge;
/// use fh6_core::{Telemetry, derived};
///
/// let mut judge = DriftJudge::new();
/// let t = Telemetry::default();
/// let d = derived::derive(&t);
/// let snap = judge.update(&t, &d, 1.0 / 60.0);
/// assert_eq!(snap.drift_state.label(), "GRIP"); // 静止时不漂移
/// ```
#[derive(Debug, Clone)]
pub struct DriftJudge {
    // ---- 平滑状态 ----
    display_angle: f32,
    g_lat_display: f32,
    g_long_display: f32,
    counter_pct: f32,
    flow_pct: f32,
    spin_risk: f32,

    // ---- 保持计时 ----
    hold_seconds: f32,
    not_holding_seconds: f32,
    long_hold_shown: bool,

    // ---- 状态机 ----
    drift_state: DriftState,
    style: Style,
    risk_label: RiskLabel,
    flow_quality: FlowQuality,
    active_hold: u32,
    entry_hold: u32,

    // ---- 帧差 ----
    prev_angle_abs: f32,
    prev_rear_slip: f32,
    prev_steer_pct: f32,
    prev_accel_pct: f32,
    prev_brake_pct: f32,
    prev_clutch_pct: f32,
    prev_handbrake_pct: f32,
    /// 上一帧的风格。用于避免「同一风格持续期间每帧都重新请求弹窗」。
    prev_style: Style,
    /// 上一帧的漂移状态。同上。
    prev_drift_state: DriftState,

    // ---- 弹窗 ----
    popup: PopupState,
    popup_cooldown: f32,
    popup_priority: u8,
    style_override: Option<Style>,
    style_override_ttl: f32,

    // ---- 事件冷却 ----
    limit_edge_cooldown: f32,

    // ---- SPIN SAVE ----
    spin_save_armed: bool,
    spin_save_peak_risk: f32,
    spin_save_peak_angle: f32,
    spin_save_recover_frames: u32,
    spin_save_cooldown: f32,

    // ---- 彩蛋 ----
    god_entry_count: u32,
    was_god_zone: bool,

    // ---- 记录本次 update 的中间量，供测试断言 ----
    last_chaos_penalty: f32,
    last_counter_deficit: f32,
}

impl Default for DriftJudge {
    fn default() -> Self {
        Self::new()
    }
}

impl DriftJudge {
    /// 创建新的判定器（全部状态归零）。
    pub fn new() -> Self {
        Self {
            display_angle: 0.0,
            g_lat_display: 0.0,
            g_long_display: 0.0,
            counter_pct: 0.0,
            flow_pct: 0.0,
            spin_risk: 0.0,
            hold_seconds: 0.0,
            not_holding_seconds: 0.0,
            long_hold_shown: false,
            drift_state: DriftState::Grip,
            style: Style::Grip,
            risk_label: RiskLabel::Safe,
            flow_quality: FlowQuality::Build,
            active_hold: 0,
            entry_hold: 0,
            prev_angle_abs: 0.0,
            prev_rear_slip: 0.0,
            prev_steer_pct: 0.0,
            prev_accel_pct: 0.0,
            prev_brake_pct: 0.0,
            prev_clutch_pct: 0.0,
            prev_handbrake_pct: 0.0,
            prev_style: Style::Grip,
            prev_drift_state: DriftState::Grip,
            popup: PopupState::default(),
            popup_cooldown: 0.0,
            popup_priority: 0,
            style_override: None,
            style_override_ttl: 0.0,
            limit_edge_cooldown: 0.0,
            spin_save_armed: false,
            spin_save_peak_risk: 0.0,
            spin_save_peak_angle: 0.0,
            spin_save_recover_frames: 0,
            spin_save_cooldown: 0.0,
            god_entry_count: 0,
            was_god_zone: false,
            last_chaos_penalty: 0.0,
            last_counter_deficit: 0.0,
        }
    }

    /// 重置全部状态（例如换车、传送、重置比赛时）。
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// 上次 update 的混乱惩罚值（调试与测试用）。
    pub fn chaos_penalty(&self) -> f32 {
        self.last_chaos_penalty
    }

    /// 上次 update 的反打缺口（调试与测试用）。
    pub fn counter_deficit(&self) -> f32 {
        self.last_counter_deficit
    }

    /// 当前平滑后的显示角度（调试与测试用）。
    pub fn display_angle(&self) -> f32 {
        self.display_angle
    }

    /// 当前平滑后的打滑风险（调试与测试用）。
    pub fn spin_risk(&self) -> f32 {
        self.spin_risk
    }

    /// 当前 FLOW 流畅度（调试与测试用）。
    pub fn flow_pct(&self) -> f32 {
        self.flow_pct
    }

    /// 当前反打量（调试与测试用）。
    pub fn counter_pct(&self) -> f32 {
        self.counter_pct
    }

    /// SPIN SAVE 是否处于武装状态（调试与测试用）。
    pub fn spin_save_armed(&self) -> bool {
        self.spin_save_armed
    }

    /// 已累计进入极限角度区的次数（调试与测试用）。
    pub fn god_entry_count(&self) -> u32 {
        self.god_entry_count
    }

    /// 当前弹窗剩余时间（调试与测试用）。
    pub fn popup_ttl(&self) -> f32 {
        self.popup.ttl
    }

    /// SPIN SAVE 的剩余冷却时间（秒，调试与测试用）。
    pub fn spin_save_cooldown(&self) -> f32 {
        self.spin_save_cooldown
    }

    /// 连续满足「已从极限恢复」条件的帧数（调试与测试用）。
    pub fn spin_save_recover_frames(&self) -> u32 {
        self.spin_save_recover_frames
    }

    /// 推进一帧。
    ///
    /// * `_t` — 原始遥测。当前实现只用 `d`（派生量已含所需的一切）；
    ///   保留此参数是为了让调用方不必在「解析后 / 派生后」之间做取舍，
    ///   也为将来需要原始字段（如 `car_ordinal` 改变时自动重置）留出接口。
    /// * `d` — 已算好的派生量。
    /// * `dt_in` — 距上一帧的真实时间（秒）。会做上下限保护。
    pub fn update(
        &mut self,
        _t: &Telemetry,
        d: &crate::derived::Derived,
        dt_in: f32,
    ) -> DriftSnapshot {
        // ---- 0. dt 下限保护 ----
        // 用真实 dt 而不是固定 0.016：掉帧时计时才不会漂移。
        // 但仍然设一个下限，避免 dt=0 或极小值导致除零/计时暴涨。
        let dt = if dt_in.is_finite() && dt_in > 0.0 {
            dt_in.clamp(0.001, 0.25)
        } else {
            // 非法 dt 回退到 60 fps 的合理默认值
            1.0 / 60.0
        };

        let speed = d.speed_kph;
        let rear_slip = d.rear_slip;

        // ---- 1. 平滑显示量 ----
        self.display_angle = smooth_alpha(self.display_angle, d.drift_angle_deg, ANGLE_SMOOTH);
        self.g_lat_display = smooth_alpha(self.g_lat_display, d.g_lateral, G_SMOOTH);
        self.g_long_display = smooth_alpha(self.g_long_display, d.g_longitudinal, G_SMOOTH);
        let abs_angle = self.display_angle.abs();

        // ---- 2. 反打量 ----
        // 注意符号约定：在本数据/符号体系下，当 display_angle 与 steer_pct
        // **同号**时才是有效反打。这与直觉可能相反，但参考实现如此，
        // 且已由实车使用验证，故沿用。
        let countering = abs_angle >= COUNTER_MIN_ANGLE && (self.display_angle * d.steer_pct) > 0.0;
        let target_counter = if countering {
            let angle_factor = clamp01((abs_angle - COUNTER_MIN_ANGLE) / COUNTER_ANGLE_SPAN);
            clamp(
                d.steer_pct.abs() * (COUNTER_GAIN_MIN + COUNTER_GAIN_SPAN * angle_factor),
                0.0,
                100.0,
            )
        } else {
            0.0
        };
        self.counter_pct = smooth_alpha(self.counter_pct, target_counter, COUNTER_SMOOTH);

        // ---- 3. 保持计时 ----
        let holding_drift =
            speed > HOLD_MIN_SPEED_KPH && abs_angle >= HOLD_MIN_ANGLE && rear_slip >= HOLD_MIN_SLIP;
        if holding_drift {
            self.hold_seconds += dt;
            self.not_holding_seconds = 0.0;
        } else {
            self.not_holding_seconds += dt;
            if self.not_holding_seconds >= HOLD_RESET_SECONDS {
                self.hold_seconds = 0.0;
                self.long_hold_shown = false;
                self.not_holding_seconds = 0.0;
            }
        }

        if self.hold_seconds >= LONG_HOLD_SECONDS && !self.long_hold_shown {
            self.request_popup(Popup::LongHold, None, Some(1.6));
            self.long_hold_shown = true;
        }

        // ---- 4. 变化率 ----
        let angle_delta = (abs_angle - self.prev_angle_abs).abs();
        let slip_delta = (rear_slip - self.prev_rear_slip).abs();
        let steer_delta = (d.steer_pct - self.prev_steer_pct).abs();
        let accel_delta = (d.throttle_pct - self.prev_accel_pct).abs();

        // ---- 5. FLOW ----
        let angle_score = clamp(
            (abs_angle - FLOW_ANGLE_BASE) / FLOW_ANGLE_SPAN * FLOW_ANGLE_MAX,
            0.0,
            FLOW_ANGLE_MAX,
        );
        let slip_score = clamp(
            (rear_slip - FLOW_SLIP_BASE) / FLOW_SLIP_SPAN * FLOW_SLIP_MAX,
            0.0,
            FLOW_SLIP_MAX,
        );
        let throttle_score = clamp(
            (d.throttle_pct - FLOW_THROTTLE_BASE) / FLOW_THROTTLE_SPAN * FLOW_THROTTLE_MAX,
            0.0,
            FLOW_THROTTLE_MAX,
        );
        let counter_score = if countering {
            clamp(
                self.counter_pct / 100.0 * FLOW_COUNTER_MAX,
                0.0,
                FLOW_COUNTER_MAX,
            )
        } else {
            0.0
        };

        let mut stability_score = 0.0;
        if holding_drift {
            stability_score += clamp(
                (1.0 - angle_delta / STAB_ANGLE_TOL) * STAB_ANGLE_MAX,
                0.0,
                STAB_ANGLE_MAX,
            );
            stability_score += clamp(
                (1.0 - slip_delta / STAB_SLIP_TOL) * STAB_SLIP_MAX,
                0.0,
                STAB_SLIP_MAX,
            );
            stability_score += clamp(
                (1.0 - accel_delta / STAB_ACCEL_TOL) * STAB_ACCEL_MAX,
                0.0,
                STAB_ACCEL_MAX,
            );
        }
        let calm_bonus = clamp(
            (self.hold_seconds - CALM_BASE_SECONDS) / CALM_SPAN_SECONDS * CALM_MAX,
            0.0,
            CALM_MAX,
        );

        let mut chaos_penalty = 0.0;
        chaos_penalty += clamp(
            (angle_delta - CHAOS_ANGLE_BASE) / CHAOS_ANGLE_SPAN * CHAOS_ANGLE_MAX,
            0.0,
            CHAOS_ANGLE_MAX,
        );
        chaos_penalty += clamp(
            (slip_delta - CHAOS_SLIP_BASE) / CHAOS_SLIP_SPAN * CHAOS_SLIP_MAX,
            0.0,
            CHAOS_SLIP_MAX,
        );
        chaos_penalty += clamp(
            (steer_delta - CHAOS_STEER_BASE) / CHAOS_STEER_SPAN * CHAOS_STEER_MAX,
            0.0,
            CHAOS_STEER_MAX,
        );
        if speed < CHAOS_LOW_SPEED_KPH {
            chaos_penalty += CHAOS_LOW_SPEED_PENALTY;
        }
        self.last_chaos_penalty = chaos_penalty;

        let target_flow = clamp(
            angle_score
                + slip_score
                + throttle_score
                + counter_score
                + stability_score
                + calm_bonus
                - chaos_penalty,
            0.0,
            100.0,
        );
        self.flow_pct = clamp(
            smooth_alpha(self.flow_pct, target_flow, FLOW_SMOOTH),
            0.0,
            100.0,
        );
        let flow_stability_pct = clamp(stability_score / STAB_TOTAL * 100.0, 0.0, 100.0);

        self.flow_quality =
            if self.flow_pct >= FLOW_LOCKED_MIN && flow_stability_pct >= FLOW_LOCKED_STABILITY {
                FlowQuality::Locked
            } else if self.flow_pct >= FLOW_SMOOTH_MIN {
                FlowQuality::Smooth
            } else if holding_drift && chaos_penalty >= CHAOS_CHASE_THRESHOLD {
                FlowQuality::Chase
            } else if holding_drift {
                FlowQuality::Flow
            } else {
                FlowQuality::Build
            };

        // ---- 6. SPIN RISK ----
        let counter_need = clamp(
            (abs_angle - COUNTER_NEED_BASE_ANGLE) / COUNTER_NEED_SPAN * 100.0,
            0.0,
            100.0,
        );
        let counter_deficit = (counter_need - self.counter_pct).max(0.0);
        self.last_counter_deficit = counter_deficit;

        let mut risk = 0.0;
        risk += clamp(
            (abs_angle - RISK_ANGLE_BASE) / RISK_ANGLE_SPAN * RISK_ANGLE_MAX,
            0.0,
            RISK_ANGLE_MAX,
        );
        risk += clamp(
            (rear_slip - RISK_SLIP_BASE) / RISK_SLIP_SPAN * RISK_SLIP_MAX,
            0.0,
            RISK_SLIP_MAX,
        );
        risk += clamp(
            (angle_delta - RISK_ANGLE_DELTA_BASE) / RISK_ANGLE_DELTA_SPAN * RISK_ANGLE_DELTA_MAX,
            0.0,
            RISK_ANGLE_DELTA_MAX,
        );
        risk += clamp(
            (slip_delta - RISK_SLIP_DELTA_BASE) / RISK_SLIP_DELTA_SPAN * RISK_SLIP_DELTA_MAX,
            0.0,
            RISK_SLIP_DELTA_MAX,
        );
        risk += clamp(
            (counter_deficit - RISK_DEFICIT_BASE) / RISK_DEFICIT_SPAN * RISK_DEFICIT_MAX,
            0.0,
            RISK_DEFICIT_MAX,
        );
        if d.throttle_pct >= RISK_PUSH_ACCEL_MIN
            && rear_slip >= RISK_PUSH_SLIP_MIN
            && abs_angle >= RISK_PUSH_ANGLE_MIN
        {
            risk += clamp(
                (d.throttle_pct - RISK_PUSH_ACCEL_MIN) / (100.0 - RISK_PUSH_ACCEL_MIN)
                    * RISK_PUSH_MAX,
                0.0,
                RISK_PUSH_MAX,
            );
        }
        if speed < RISK_LOWSPEED_KPH && abs_angle >= RISK_LOWSPEED_ANGLE_MIN {
            risk += clamp(
                (RISK_LOWSPEED_ANGLE_MIN - speed) / (RISK_LOWSPEED_ANGLE_MIN + 2.0)
                    * RISK_LOWSPEED_MAX,
                0.0,
                RISK_LOWSPEED_MAX,
            );
        }
        if holding_drift
            && matches!(self.flow_quality, FlowQuality::Smooth | FlowQuality::Locked)
            && countering
        {
            risk -= clamp(
                (self.flow_pct - RISK_RELIEF_FLOW_BASE) / RISK_RELIEF_FLOW_SPAN * RISK_RELIEF_MAX,
                0.0,
                RISK_RELIEF_MAX,
            );
        }
        if speed < 8.0 && abs_angle < 16.0 {
            risk *= RISK_LOW_ACTIVITY_SCALE;
        }

        self.spin_risk = clamp(
            smooth_alpha(self.spin_risk, clamp(risk, 0.0, 100.0), RISK_SMOOTH),
            0.0,
            100.0,
        );

        let prev_risk_label = self.risk_label;
        self.risk_label = if self.spin_risk >= RISK_LABEL_MAX {
            RiskLabel::Max
        } else if self.spin_risk >= RISK_LABEL_RISK {
            RiskLabel::Risk
        } else if self.spin_risk >= RISK_LABEL_EDGE {
            RiskLabel::Edge
        } else {
            RiskLabel::Safe
        };

        // ---- 7. LIMIT EDGE 事件 ----
        if self.limit_edge_cooldown > 0.0 {
            self.limit_edge_cooldown = (self.limit_edge_cooldown - dt).max(0.0);
        }
        // 只在「从 SAFE 刚跨入 EDGE」的那一帧请求，避免在 EDGE 期间反复弹出。
        if self.risk_label == RiskLabel::Edge
            && prev_risk_label == RiskLabel::Safe
            && holding_drift
            && self.limit_edge_cooldown <= 0.0
            && self.request_popup(Popup::LimitEdge, None, Some(1.2))
        {
            self.limit_edge_cooldown = 3.2;
        }

        // ---- 8. 风格 ----
        let base_style = if self.risk_label == RiskLabel::Max {
            Style::Danger
        } else if self.risk_label == RiskLabel::Risk {
            Style::Edge
        } else if rear_slip >= 1.35 && d.throttle_pct >= 50.0 && abs_angle >= 28.0 {
            Style::Wild
        } else if self.flow_quality == FlowQuality::Locked && abs_angle >= 24.0 {
            Style::Locked
        } else if self.flow_pct >= 76.0 && abs_angle >= 30.0 {
            Style::Deep
        } else if self.flow_pct >= 56.0 && rear_slip >= 0.65 {
            Style::Smooth
        } else if holding_drift {
            Style::Clean
        } else if abs_angle >= 10.0 && speed > 15.0 {
            Style::Entry
        } else {
            Style::Grip
        };

        if self.style_override_ttl > 0.0 {
            self.style_override_ttl = (self.style_override_ttl - dt).max(0.0);
            self.style = self.style_override.unwrap_or(base_style);
            if self.style_override_ttl <= 0.0 {
                self.style_override = None;
            }
        } else {
            self.style = base_style;
        }

        // ---- 9. SPIN SAVE ----
        if self.spin_save_cooldown > 0.0 {
            self.spin_save_cooldown = (self.spin_save_cooldown - dt).max(0.0);
        }

        let spin_save_danger = self.spin_risk >= SAVE_ARM_RISK
            && abs_angle >= SAVE_ARM_ANGLE
            && speed > SAVE_ARM_SPEED_KPH
            && rear_slip >= SAVE_ARM_SLIP;
        if spin_save_danger {
            self.spin_save_armed = true;
            self.spin_save_peak_risk = self.spin_save_peak_risk.max(self.spin_risk);
            self.spin_save_peak_angle = self.spin_save_peak_angle.max(abs_angle);
            self.spin_save_recover_frames = 0;
        }

        if self.spin_save_armed {
            self.spin_save_peak_risk = self.spin_save_peak_risk.max(self.spin_risk);
            self.spin_save_peak_angle = self.spin_save_peak_angle.max(abs_angle);

            let recovered = self.spin_save_peak_risk >= SAVE_PEAK_RISK
                && self.spin_risk <= SAVE_RECOVER_RISK
                && (SAVE_RECOVER_ANGLE_MIN..=SAVE_RECOVER_ANGLE_MAX).contains(&abs_angle)
                && speed > SAVE_ARM_SPEED_KPH
                && rear_slip >= SAVE_RECOVER_SLIP;
            if recovered {
                self.spin_save_recover_frames += 1;
            } else {
                self.spin_save_recover_frames = 0;
            }

            if self.spin_save_recover_frames >= SAVE_RECOVER_FRAMES
                && self.spin_save_cooldown <= 0.0
            {
                self.request_popup(Popup::SpinSave, None, Some(2.3));
                self.style_override = Some(Style::Save);
                self.style_override_ttl = SAVE_STYLE_SECONDS;
                self.style = Style::Save;
                self.spin_save_cooldown = SAVE_COOLDOWN_SECONDS;
                self.spin_save_armed = false;
                self.spin_save_peak_risk = 0.0;
                self.spin_save_peak_angle = 0.0;
                self.spin_save_recover_frames = 0;
            }
        }

        if speed < SAVE_DISARM_SPEED_KPH || abs_angle < SAVE_DISARM_ANGLE {
            self.spin_save_armed = false;
            self.spin_save_peak_risk = 0.0;
            self.spin_save_peak_angle = 0.0;
            self.spin_save_recover_frames = 0;
        }

        // ---- 10. 漂移状态机 ----
        let drift_like = (abs_angle > 8.0 && speed > 15.0)
            || rear_slip > 0.35
            || (d.steer_pct.abs() > 28.0 && d.throttle_pct > 20.0 && speed > 20.0)
            || d.handbrake_pct > 12.0
            || (d.brake_pct > 20.0 && speed > 25.0);
        if drift_like {
            self.active_hold = ACTIVE_HOLD_FRAMES;
        } else {
            self.active_hold = self.active_hold.saturating_sub(1);
        }

        let entering_now = ((d.handbrake_pct > 14.0 && speed > 18.0)
            || (d.brake_pct > 26.0 && speed > 28.0 && abs_angle < 14.0)
            || (d.steer_pct.abs() > 42.0
                && d.throttle_pct > 10.0
                && speed > 22.0
                && abs_angle < 12.0)
            || (abs_angle > 12.0 && self.prev_angle_abs < 6.0 && speed > 22.0))
            && rear_slip < 1.0;
        self.entry_hold = self.entry_hold.saturating_sub(1);
        if entering_now {
            self.entry_hold = ENTRY_HOLD_FRAMES;
        }

        self.drift_state = if speed < STATE_IDLE_SPEED_KPH && abs_angle < STATE_IDLE_ANGLE {
            DriftState::Grip
        } else if self.risk_label == RiskLabel::Max && speed > STATE_SPIN_SPEED_KPH {
            DriftState::Spin
        } else if self.active_hold > 0
            && rear_slip >= STATE_SMOKE_SLIP
            && d.throttle_pct >= STATE_SMOKE_ACCEL
        {
            DriftState::Smoke
        } else if self.active_hold > 0
            && abs_angle >= STATE_ANGLE_MIN
            && speed > STATE_ANGLE_SPEED_KPH
        {
            DriftState::Angle
        } else if self.entry_hold > 0 {
            DriftState::Entry
        } else if self.active_hold > 0 {
            DriftState::Hold
        } else {
            DriftState::Grip
        };

        // ---- 11. GOD / CAT 彩蛋 ----
        let in_god_zone = abs_angle > 60.0;
        if in_god_zone && !self.was_god_zone {
            self.god_entry_count += 1;
        }
        self.was_god_zone = in_god_zone;
        let god_word = if self.god_entry_count > 0 && self.god_entry_count % 10 == 0 {
            "CAT"
        } else {
            "GOD"
        };

        // ---- 12. 弹窗推进 ----
        if self.popup_cooldown > 0.0 {
            self.popup_cooldown = (self.popup_cooldown - dt).max(0.0);
        }
        if self.popup.ttl > 0.0 {
            self.popup.ttl = (self.popup.ttl - dt).max(0.0);
        } else {
            self.popup.popup = None;
            self.popup_priority = 0;
        }
        if self.popup.operation_ttl > 0.0 {
            self.popup.operation_ttl = (self.popup.operation_ttl - dt).max(0.0);
        } else {
            self.popup.operation = None;
            self.popup.operation_total_ttl = 0.0;
        }

        // ---- 13. 操作提示（手刹 / 离合 / 刹车） ----
        let handbrake_hit =
            d.handbrake_pct >= 38.0 && self.prev_handbrake_pct < 18.0 && speed > 8.0;
        let clutch_kick = d.clutch_pct >= 58.0
            && self.prev_clutch_pct < 22.0
            && d.throttle_pct >= 18.0
            && speed > 8.0;
        let foot_brake = d.brake_pct >= 48.0
            && self.prev_brake_pct < 20.0
            && d.handbrake_pct < 18.0
            && speed > 10.0;

        if self.popup.operation_ttl <= 0.0 {
            let trigger = if handbrake_hit {
                Some(OperationPopup::Handbrake)
            } else if clutch_kick {
                Some(OperationPopup::Clutch)
            } else if foot_brake {
                Some(OperationPopup::Brake)
            } else {
                None
            };
            if let Some(op) = trigger {
                self.popup.operation = Some(op);
                self.popup.operation_ttl = OPERATION_POPUP_SECONDS;
                self.popup.operation_total_ttl = OPERATION_POPUP_SECONDS;
                self.style_override = Some(op.style_override());
                self.style_override_ttl = OPERATION_STYLE_SECONDS;
                self.style = op.style_override();
            }
        }

        // ---- 14. 事件弹窗（优先级抢占） ----
        //
        // ⚠️ 必须用「上一帧状态不同」做守卫，否则只要风格持续为 LOCKED/DEEP、
        // 或状态持续为 SMOKE/ENTRY，就会**每帧都请求一次弹窗**，
        // 导致弹窗被无限续期（永远不消失）且冷却被反复刷新。
        // 参考实现用 `last_style_label != "LOCKED"` 这类比较来防重，此处等价实现。
        if abs_angle >= 46.0 && self.prev_angle_abs < 46.0 {
            self.request_popup(Popup::BigAngle, None, Some(1.2));
        } else if self.popup_cooldown <= 0.0 {
            if self.style == Style::Locked && self.prev_style != Style::Locked {
                self.request_popup(Popup::FlowLock, None, Some(1.8));
            } else if self.style == Style::Deep && self.prev_style != Style::Deep {
                self.request_popup(Popup::DeepLine, None, Some(1.4));
            } else if self.drift_state == DriftState::Smoke
                && self.prev_drift_state != DriftState::Smoke
            {
                self.request_popup(Popup::SmokeRun, None, Some(1.4));
            } else if self.drift_state == DriftState::Entry
                && self.prev_drift_state != DriftState::Entry
            {
                self.request_popup(Popup::Entry, None, Some(1.1));
            }
        }

        // ---- 15. 保存帧差 ----
        self.prev_angle_abs = abs_angle;
        self.prev_rear_slip = rear_slip;
        self.prev_steer_pct = d.steer_pct;
        self.prev_accel_pct = d.throttle_pct;
        self.prev_brake_pct = d.brake_pct;
        self.prev_clutch_pct = d.clutch_pct;
        self.prev_handbrake_pct = d.handbrake_pct;
        self.prev_style = self.style;
        self.prev_drift_state = self.drift_state;

        DriftSnapshot {
            display_angle: self.display_angle,
            g_lateral: self.g_lat_display,
            g_longitudinal: self.g_long_display,
            counter_pct: self.counter_pct,
            hold_seconds: self.hold_seconds,
            flow_pct: self.flow_pct,
            flow_stability_pct,
            flow_quality: self.flow_quality,
            spin_risk: self.spin_risk,
            risk_label: self.risk_label,
            style: self.style,
            drift_state: self.drift_state,
            state_reason: self.drift_state.reason(),
            holding_drift,
            countering,
            god_word,
            god_entry_count: self.god_entry_count,
            popup: self.popup,
        }
    }

    /// 请求显示一个事件弹窗。
    ///
    /// 优先级规则：高优先级可以抢占低优先级；低优先级**不会**打断 hero 时刻。
    /// 返回 `true` 表示本次请求被接受。
    fn request_popup(
        &mut self,
        p: Popup,
        seconds: Option<f32>,
        cooldown_seconds: Option<f32>,
    ) -> bool {
        let priority = p.priority();
        let current_priority = if self.popup.ttl > 0.0 {
            self.popup_priority
        } else {
            0
        };
        if self.popup.ttl > 0.0 && priority < current_priority {
            return false;
        }

        let seconds = seconds.unwrap_or_else(|| p.default_seconds());
        self.popup.popup = Some(p);
        self.popup_priority = priority;
        self.popup.ttl = seconds.max(0.001);
        if let Some(cd) = cooldown_seconds {
            self.popup_cooldown = self.popup_cooldown.max(cd.max(0.0));
        }
        true
    }
}

/// 用指定系数向目标值做指数平滑。
#[inline]
fn smooth_alpha(current: f32, target: f32, alpha: f32) -> f32 {
    if !current.is_finite() {
        return target;
    }
    if !target.is_finite() {
        return current;
    }
    current + (target - current) * alpha
}

/// 钳制到 0.0–1.0。
#[inline]
fn clamp01(v: f32) -> f32 {
    if v.is_nan() {
        0.0
    } else {
        v.clamp(0.0, 1.0)
    }
}

/// 三值钳制，NaN 回退到 `min`。
#[inline]
fn clamp(v: f32, min: f32, max: f32) -> f32 {
    if v.is_nan() {
        min
    } else {
        v.clamp(min, max)
    }
}
