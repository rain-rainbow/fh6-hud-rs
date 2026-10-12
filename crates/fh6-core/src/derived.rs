//! 派生量计算：把原始遥测换算成 HUD 真正要显示的东西。
//!
//! 本模块只做「数学与单位换算」，不做滤波、不做状态机、不碰渲染。
//! 三件事分开的原因：**业务逻辑要能被单元测试**，而不需要启动窗口。
//!
//! # 与参考实现的语义差异声明
//!
//! 这里对「漂移角」的定义是：**速度矢量相对世界 Z 轴的夹角**，
//! 即 `atan2(VelocityX, VelocityZ)`。
//!
//! ⚠️ 这**不是**轮胎的侧偏角（slip angle），也**不是**车头朝向与速度方向的
//! 夹角。它是一个和车辆朝向无关的「世界坐标系姿态量」。参考实现
//! （093_Drift_HUD）用的正是这个定义，本项目为保持「手感一致」沿用，
//! 并在 `README` 中明确标注，避免使用者误以为它是专业意义上的漂移角。
//!
//! 侧偏角请用 [`Telemetry::wheel_rl`] 的 `slip_angle` 字段（原始量）。

use crate::pack::*;
use crate::telemetry::Telemetry;

/// 计算漂移角所需的最低速度（m/s）。
///
/// 低于此速度时不计算角度：静止或极低速时 `atan2(0,0)` 无意义，
/// 且速度为 0 时方向本身就没有定义。
const MIN_SPEED_FOR_ANGLE_MPS: f32 = 4.0;

/// 计算漂移角所需的最低水平速度分量之和（m/s）。
const MIN_HORIZONTAL_SPEED_MPS: f32 = 0.5;

/// 漂移角的显示钳制范围（度）。
///
/// 超过 ±90° 意味着车在倒着走，"漂移角" 会跳到 ±180 附近产生视觉抖动，
/// 因此统一钳到 ±90。
pub const DRIFT_ANGLE_CLAMP_DEG: f32 = 90.0;

/// 横/纵向 G 值的显示钳制范围。
pub const G_LATERAL_CLAMP: f32 = 2.5;
/// 纵向 G 值的显示钳制范围。
pub const G_LONGITUDINAL_CLAMP: f32 = 2.5;
/// 垂直 G 值的显示钳制范围（赛道起伏更剧烈，给更大范围）。
pub const G_VERTICAL_CLAMP: f32 = 3.0;

/// 一帧的派生量。全部为「已换算、已钳制」的展示值。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Derived {
    /// 速度（km/h）。
    pub speed_kph: f32,
    /// 漂移角（度），范围 ±90。符号表示侧滑方向。
    pub drift_angle_deg: f32,
    /// 纵向 G（加速为正、制动为负）。
    pub g_longitudinal: f32,
    /// 横向 G。**正 = 右转，负 = 左转**（ISO 8855 / 竞赛遥测约定）。
    ///
    /// 符号约定详见 [`g_forces`] 的文档。
    pub g_lateral: f32,
    /// 垂直 G（上方向为正）。
    pub g_vertical: f32,
    /// 后轴平均综合滑移量（绝对值平均）。
    pub rear_slip: f32,
    /// 油门百分比，0..100。
    pub throttle_pct: f32,
    /// 刹车百分比，0..100。
    pub brake_pct: f32,
    /// 离合百分比，0..100。
    pub clutch_pct: f32,
    /// 手刹百分比，0..100。
    pub handbrake_pct: f32,
    /// 方向盘百分比，-100..100（负为左）。
    pub steer_pct: f32,
    /// 当前转速 / 最大转速，0..1。
    pub rpm_ratio: f32,
    /// 挡位原始值（0 表示倒挡）。
    ///
    /// 这里**刻意不存字符串**：`Derived` 是每帧都要重新计算并传进渲染热路径的
    /// 结构体，保持纯数值 + `Copy` 可以完全避免堆分配。
    /// 需要显示文本时调用 [`Derived::gear_display`]。
    pub gear_raw: u8,
    /// 传动类型显示文本（`&'static str`，不涉及分配）。
    pub drivetrain_label: &'static str,
}

impl Derived {
    /// 挡位显示文本：0 → `"R"`，其余为数字。
    ///
    /// 写入调用方提供的缓冲区，避免在渲染热路径里分配堆内存。
    /// 返回写入的切片。
    ///
    /// ```
    /// use fh6_core::Derived;
    /// let mut buf = [0u8; 4];
    /// let d = Derived { gear_raw: 0, ..Default::default() };
    /// assert_eq!(d.gear_display(&mut buf), "R");
    /// let d = Derived { gear_raw: 4, ..Default::default() };
    /// assert_eq!(d.gear_display(&mut buf), "4");
    /// ```
    pub fn gear_display<'a>(&self, buf: &'a mut [u8; 4]) -> &'a str {
        if self.gear_raw == 0 {
            buf[0] = b'R';
            // SAFETY 之外的替代方案：用 from_utf8 并处理错误。
            // 这里写入的字节恒为 ASCII，用 from_utf8 不会有运行期失败。
            return core::str::from_utf8(&buf[..1]).unwrap_or("?");
        }
        // 最多 3 位数字（255），4 字节缓冲区足够
        let mut n = self.gear_raw;
        let mut i = 0usize;
        if n >= 100 {
            buf[i] = b'0' + (n / 100);
            n %= 100;
            i += 1;
        }
        if i > 0 || n >= 10 {
            buf[i] = b'0' + (n / 10);
            n %= 10;
            i += 1;
        }
        buf[i] = b'0' + n;
        i += 1;
        core::str::from_utf8(&buf[..i]).unwrap_or("?")
    }
}

/// 计算一帧的全部派生量。
///
/// 纯函数：同样的输入必然得到同样的输出，不读写任何全局状态。
pub fn derive(t: &Telemetry) -> Derived {
    let (g_long, g_lat, g_vert) = g_forces(t);

    Derived {
        speed_kph: t.speed_mps * MPS_TO_KPH,
        drift_angle_deg: drift_angle_deg(t),
        g_longitudinal: clamp(g_long, -G_LONGITUDINAL_CLAMP, G_LONGITUDINAL_CLAMP),
        g_lateral: clamp(g_lat, -G_LATERAL_CLAMP, G_LATERAL_CLAMP),
        g_vertical: clamp(g_vert, -G_VERTICAL_CLAMP, G_VERTICAL_CLAMP),
        rear_slip: t.rear_combined_slip(),
        throttle_pct: t.accel_raw as f32 / INPUT_U8_MAX * 100.0,
        brake_pct: t.brake_raw as f32 / INPUT_U8_MAX * 100.0,
        clutch_pct: t.clutch_raw as f32 / INPUT_U8_MAX * 100.0,
        handbrake_pct: t.handbrake_raw as f32 / INPUT_U8_MAX * 100.0,
        steer_pct: clamp(t.steer_raw as f32 / STEER_S8_MAX * 100.0, -100.0, 100.0),
        rpm_ratio: if t.engine_max_rpm > 1.0 {
            clamp(t.current_engine_rpm / t.engine_max_rpm, 0.0, 1.0)
        } else {
            0.0
        },
        gear_raw: t.gear_raw,
        drivetrain_label: drivetrain_label(t.drivetrain_type),
    }
}

/// 漂移角（度）：速度矢量相对世界 Z 轴的夹角，钳制到 ±90。
///
/// 低速或水平速度分量过小时返回 0，避免静止时角度乱跳。
pub fn drift_angle_deg(t: &Telemetry) -> f32 {
    let vx = t.velocity[0];
    let vz = t.velocity[2];

    if !vx.is_finite() || !vz.is_finite() {
        return 0.0;
    }
    if t.speed_mps <= MIN_SPEED_FOR_ANGLE_MPS {
        return 0.0;
    }
    if vx.abs() + vz.abs() <= MIN_HORIZONTAL_SPEED_MPS {
        return 0.0;
    }

    // 注意参数顺序是 (x, z)：以世界 Z 轴为「正前方」基准。
    let deg = vx.atan2(vz).to_degrees();
    clamp(deg, -DRIFT_ANGLE_CLAMP_DEG, DRIFT_ANGLE_CLAMP_DEG)
}

/// 计算纵向 / 横向 / 垂直 G 值。
///
/// # 算法
///
/// 官方给的 [`crate::pack::ACCELERATION`] 是**世界坐标系**加速度。要得到驾驶员
/// 体感的「纵向 / 横向」分量，必须把加速度投影到**水平速度方向**上：
///
/// 1. 取水平速度 `(vx, vz)` 并归一化得到前进方向 `(fx, fz)`；
/// 2. 纵向加速度 = 加速度在前进方向上的投影 `ax*fx + az*fz`；
/// 3. 横向加速度 = 与前进方向垂直的分量 `az*fx - ax*fz`；
/// 4. 垂直加速度直接取 Y 分量。
///
/// # ⚠️ 横向 G 的符号约定（本项目被测试抓错过一次，务必注意）
///
/// 本函数采用**右转为正**，这是竞赛遥测与车辆动力学标准的通行约定
/// （ISO 8855 车体坐标系：X 向右、Y 向上、Z 向前，绕 Y 轴正方向旋转为右转）。
///
/// 代数验证（见集成测试 `横向G符号约定为右转正`）：
///
/// ```text
/// 车沿 +Z 前进 → fx = 0, fz = 1
/// 纯转弯时加速度沿 ±X 方向
/// g_lat = (az*0 - ax*1)/g = -ax/g
///
/// 右转 → 向心加速度指向 +X → ax > 0 → g_lat < 0   ← 与约定不符
/// ```
///
/// 因此必须写成 **`ax*fz - az*fx`**，这样右转 → `ax > 0` → `g_lat > 0` ✔
///
/// 换句话说：`ax*fz - az*fx` 是「向心加速度方向」的横向分量，
/// 而 `az*fx - ax*fz` 得到的是相反的符号（离心方向）。
/// 两者物理上互为反号，选哪个纯粹是显示约定；本项目选前者并与
/// `G_LATERAL` 的正负含义在文档中固定下来。
///
/// 速度接近 0 时前进方向无定义，此时退化为「按世界轴直接取 X/Z」，
/// 保证 G 表不会突然归零。
pub fn g_forces(t: &Telemetry) -> (f32, f32, f32) {
    let ax = t.acceleration[0];
    let ay = t.acceleration[1];
    let az = t.acceleration[2];

    let (ax, ay, az) = if ax.is_finite() && ay.is_finite() && az.is_finite() {
        (ax, ay, az)
    } else {
        (0.0, 0.0, 0.0)
    };

    let g_vert = ay / STANDARD_GRAVITY;

    let vx = t.velocity[0];
    let vz = t.velocity[2];
    let horiz = (vx * vx + vz * vz).sqrt();

    let (g_long, g_lat) = if horiz > MIN_HORIZONTAL_SPEED_MPS && horiz.is_finite() {
        let fx = vx / horiz;
        let fz = vz / horiz;
        // 纵向：加速度在前进方向上的投影
        let long = (ax * fx + az * fz) / STANDARD_GRAVITY;
        // 横向：右转为正。写成 az*fx - ax*fz 会导致左右颠倒。
        let lat = (ax * fz - az * fx) / STANDARD_GRAVITY;
        (long, lat)
    } else {
        (ax / STANDARD_GRAVITY, az / STANDARD_GRAVITY)
    };

    (g_long, g_lat, g_vert)
}

/// 传动类型 → 显示文本。
pub fn drivetrain_label(drivetrain_type: i32) -> &'static str {
    match drivetrain_type {
        DRIVETRAIN_FWD => "FWD",
        DRIVETRAIN_RWD => "RWD",
        DRIVETRAIN_AWD => "AWD",
        _ => "?",
    }
}

/// 车辆等级 → 显示文本。
///
/// ⚠️ 这个映射是**社区通行约定**，官方 FH6 文档未给出 `CarClass` 的枚举含义。
/// 越界取值返回 `"?"` 而不是 panic。
pub fn car_class_label(car_class: i32) -> &'static str {
    match car_class {
        0 => "D",
        1 => "C",
        2 => "B",
        3 => "A",
        4 => "S1",
        5 => "S2",
        6 => "X",
        7 => "E",
        _ => "?",
    }
}

/// 通用钳制。`min > max` 时行为回退为返回 `min`，不会 panic。
#[inline]
pub fn clamp(v: f32, min: f32, max: f32) -> f32 {
    if v.is_nan() {
        return min;
    }
    if v < min {
        min
    } else if v > max {
        max
    } else {
        v
    }
}

/// 指数平滑（低通滤波），用于消除数值抖动。
///
/// `alpha` 是「新值权重」：
/// * `alpha = 1.0` → 完全跟随新值（不平滑）；
/// * `alpha = 0.0` → 完全不变；
/// * 常规取 0.2 左右。
///
/// 首次调用（`prev` 非有限）时直接返回 `next`，避免从 0 缓慢爬升。
#[inline]
pub fn smooth(prev: f32, next: f32, alpha: f32) -> f32 {
    if !prev.is_finite() {
        return next;
    }
    if !next.is_finite() {
        return prev;
    }
    let a = clamp(alpha, 0.0, 1.0);
    prev + (next - prev) * a
}
