//! 派生量计算的集成测试。
//!
//! 这些测试的意义：**业务数学不需要启动窗口、也不需要连着游戏就能验证**。
//! 这是把 `fh6-core` 从渲染层拆出来的直接收益。
//!
//! 测试函数名用中文描述意图，可读性更好；因此关掉 `non_snake_case` 检查。
#![allow(non_snake_case)]

use fh6_core::derived::{self, clamp, smooth, Derived};
use fh6_core::{Telemetry, Wheel};

/// 构造一帧全部为零的遥测作为基准。
fn base() -> Telemetry {
    Telemetry::default()
}

#[test]
fn 速度换算m每秒到公里每小时() {
    let mut t = base();
    t.speed_mps = 10.0;
    let d = derived::derive(&t);
    assert!((d.speed_kph - 36.0).abs() < 1e-4, "10 m/s 应为 36 km/h");
}

#[test]
fn 静止时漂移角为零而不是NaN() {
    let t = base(); // speed = 0
    let d = derived::derive(&t);
    assert_eq!(d.drift_angle_deg, 0.0);
    assert!(d.drift_angle_deg.is_finite());
}

#[test]
fn 低速时漂移角被抑制() {
    let mut t = base();
    t.speed_mps = 3.0; // 低于 4 m/s 阈值
    t.velocity = [3.0, 0.0, 3.0];
    let d = derived::derive(&t);
    assert_eq!(d.drift_angle_deg, 0.0, "低速不应计算漂移角");
}

#[test]
fn 漂移角为速度矢量相对世界Z轴的夹角() {
    let mut t = base();
    t.speed_mps = 20.0;
    // 速度纯 +Z 方向 → 夹角 0
    t.velocity = [0.0, 0.0, 20.0];
    assert!(derived::derive(&t).drift_angle_deg.abs() < 1e-4);

    // 速度纯 +X 方向 → 夹角 90
    t.velocity = [20.0, 0.0, 0.0];
    assert!((derived::derive(&t).drift_angle_deg - 90.0).abs() < 1e-3);

    // 速度纯 -X 方向 → 夹角 -90
    t.velocity = [-20.0, 0.0, 0.0];
    assert!((derived::derive(&t).drift_angle_deg + 90.0).abs() < 1e-3);

    // 45 度
    t.velocity = [14.142, 0.0, 14.142];
    assert!((derived::derive(&t).drift_angle_deg - 45.0).abs() < 1e-2);
}

#[test]
fn 漂移角被钳制在正负90度() {
    let mut t = base();
    t.speed_mps = 20.0;
    // 车在倒着走：atan2 会得到接近 ±180 的角度，必须被钳制
    t.velocity = [1.0, 0.0, -20.0];
    let d = derived::derive(&t);
    assert!(d.drift_angle_deg.abs() <= derived::DRIFT_ANGLE_CLAMP_DEG);
    assert!((-90.0..=90.0).contains(&d.drift_angle_deg));
}

#[test]
fn 纵向G在直线加速时为正() {
    let mut t = base();
    t.speed_mps = 30.0;
    t.velocity = [0.0, 0.0, 30.0]; // 沿 +Z 前进
    t.acceleration = [0.0, 0.0, 9.80665]; // 沿 +Z 加速 1G
    let d = derived::derive(&t);
    assert!((d.g_longitudinal - 1.0).abs() < 1e-3, "应得到 +1G");
    assert!(d.g_lateral.abs() < 1e-3, "直线行驶横向 G 应为 0");
}

#[test]
fn 制动时纵向G为负() {
    let mut t = base();
    t.speed_mps = 30.0;
    t.velocity = [0.0, 0.0, 30.0];
    t.acceleration = [0.0, 0.0, -9.80665];
    let d = derived::derive(&t);
    assert!((d.g_longitudinal + 1.0).abs() < 1e-3, "制动应得到 -1G");
}

#[test]
fn 横向G符号约定为右转正() {
    // 符号约定：**右转为正**（ISO 8855 / 竞赛遥测通行约定）。
    //
    // 车沿 +Z 前进时，纯转弯的加速度沿 ±X：
    //   g_lat = (ax*fz - az*fx)/g = ax/g
    // 右转 → 向心加速度指向 +X → g_lat 为正。
    let mut t = base();
    t.speed_mps = 30.0;
    t.velocity = [0.0, 0.0, 30.0];

    // 右转：加速度指向 +X
    t.acceleration = [9.80665, 0.0, 0.0];
    let d = derived::derive(&t);
    assert!(d.g_lateral > 0.9, "右转横向 G 应为正, 实际 {}", d.g_lateral);

    // 左转：加速度指向 -X，横向 G 应为负
    t.acceleration = [-9.80665, 0.0, 0.0];
    let d = derived::derive(&t);
    assert!(
        d.g_lateral < -0.9,
        "左转横向 G 应为负, 实际 {}",
        d.g_lateral
    );

    // 左右必须严格对称
    t.acceleration = [4.0, 0.0, 0.0];
    let right = derived::derive(&t).g_lateral;
    t.acceleration = [-4.0, 0.0, 0.0];
    let left = derived::derive(&t).g_lateral;
    assert!((left + right).abs() < 1e-6, "左右横向 G 应对称");
}

#[test]
fn 垂直G来自Y轴加速度() {
    let mut t = base();
    t.acceleration = [0.0, 9.80665, 0.0];
    let d = derived::derive(&t);
    assert!((d.g_vertical - 1.0).abs() < 1e-3);
}

#[test]
fn G值被钳制在显示范围内() {
    let mut t = base();
    t.speed_mps = 30.0;
    t.velocity = [0.0, 0.0, 30.0];
    t.acceleration = [0.0, 1000.0, 1000.0]; // 荒谬的大加速度
    let d = derived::derive(&t);
    assert!(d.g_longitudinal.abs() <= derived::G_LONGITUDINAL_CLAMP);
    assert!(d.g_vertical.abs() <= derived::G_VERTICAL_CLAMP);
}

#[test]
fn 输入百分比换算正确() {
    let mut t = base();
    t.accel_raw = 255;
    t.brake_raw = 0;
    t.clutch_raw = 128;
    t.handbrake_raw = 51;
    t.steer_raw = 127;
    let d = derived::derive(&t);
    assert!((d.throttle_pct - 100.0).abs() < 1e-4);
    assert_eq!(d.brake_pct, 0.0);
    assert!((d.clutch_pct - 128.0 / 255.0 * 100.0).abs() < 1e-4);
    assert!((d.handbrake_pct - 20.0).abs() < 1e-4);
    assert!((d.steer_pct - 100.0).abs() < 1e-4, "满右打应为 +100%");
}

#[test]
fn 方向盘用127而不是128做满量程() {
    // S8 范围是 -128..127。若误用 128 作除数，满右打会得到 99.2%。
    let mut t = base();
    t.steer_raw = 127;
    let d = derived::derive(&t);
    assert!((d.steer_pct - 100.0).abs() < 1e-6);

    // -128 会略微超过 -100%，必须被钳制
    t.steer_raw = -128;
    let d = derived::derive(&t);
    assert_eq!(d.steer_pct, -100.0, "超出部分应被钳制到 -100%");
}

#[test]
fn 后轴滑移取绝对值平均() {
    let mut t = base();
    t.wheel_rl = Wheel {
        combined_slip: -0.8,
        ..Default::default()
    };
    t.wheel_rr = Wheel {
        combined_slip: 0.4,
        ..Default::default()
    };
    // 倒车打滑时滑移率为负，但打滑程度应看幅值
    assert!((t.rear_combined_slip() - 0.6).abs() < 1e-6);
}

#[test]
fn 挡位0显示为R其余为数字() {
    let mut buf = [0u8; 4];

    let d = Derived {
        gear_raw: 0,
        ..Default::default()
    };
    assert_eq!(d.gear_display(&mut buf), "R");

    for (raw, expect) in [
        (1u8, "1"),
        (9, "9"),
        (10, "10"),
        (12, "12"),
        (100, "100"),
        (255, "255"),
    ] {
        let d = Derived {
            gear_raw: raw,
            ..Default::default()
        };
        assert_eq!(d.gear_display(&mut buf), expect, "挡位 {raw} 显示错误");
    }
}

#[test]
fn 转速比在最大转速为零时不产生除零() {
    let mut t = base();
    t.engine_max_rpm = 0.0;
    t.current_engine_rpm = 5000.0;
    let d = derived::derive(&t);
    assert_eq!(d.rpm_ratio, 0.0);
    assert!(d.rpm_ratio.is_finite());
}

#[test]
fn 传动类型标签正确() {
    let mut t = base();
    t.drivetrain_type = 0;
    assert_eq!(derived::derive(&t).drivetrain_label, "FWD");
    t.drivetrain_type = 1;
    assert_eq!(derived::derive(&t).drivetrain_label, "RWD");
    t.drivetrain_type = 2;
    assert_eq!(derived::derive(&t).drivetrain_label, "AWD");
    t.drivetrain_type = 99;
    assert_eq!(derived::derive(&t).drivetrain_label, "?");
}

#[test]
fn NaN输入不会让派生量崩溃() {
    let mut t = base();
    t.speed_mps = f32::NAN;
    t.velocity = [f32::NAN, f32::NAN, f32::NAN];
    t.acceleration = [f32::NAN, f32::NAN, f32::NAN];
    let d = derived::derive(&t);
    // 漂移角必须被兜底为 0
    assert_eq!(d.drift_angle_deg, 0.0);
    // G 值也必须有限（NaN 被 clamp 吞掉）
    assert!(d.g_longitudinal.is_finite());
    assert!(d.g_lateral.is_finite());
    assert!(d.g_vertical.is_finite());
}

#[test]
fn clamp与smooth的边界行为() {
    assert_eq!(clamp(5.0, 0.0, 1.0), 1.0);
    assert_eq!(clamp(-5.0, 0.0, 1.0), 0.0);
    assert_eq!(clamp(0.5, 0.0, 1.0), 0.5);
    assert_eq!(clamp(f32::NAN, 0.0, 1.0), 0.0, "NaN 应回退到 min");

    // alpha=1 完全跟随
    assert_eq!(smooth(0.0, 10.0, 1.0), 10.0);
    // alpha=0 完全不动
    assert_eq!(smooth(3.0, 10.0, 0.0), 3.0);
    // 首次调用（prev 非有限）直接取新值
    assert_eq!(smooth(f32::NAN, 7.0, 0.2), 7.0);
}

#[test]
fn 零分配的派生量是Copy类型() {
    // 编译期保证：Derived 必须是 Copy，才能在渲染热路径里自由按值传递。
    fn assert_copy<T: Copy>() {}
    assert_copy::<Derived>();
}

#[test]
fn 解析编码往返后派生量一致() {
    let t = Telemetry {
        speed_mps: 25.5,
        velocity: [12.0, 0.5, 22.0],
        acceleration: [-3.0, 9.0, 4.0],
        accel_raw: 200,
        steer_raw: -64,
        gear_raw: 3,
        wheel_rl: Wheel {
            combined_slip: 0.9,
            ..Default::default()
        },
        wheel_rr: Wheel {
            combined_slip: 0.7,
            ..Default::default()
        },
        ..Default::default()
    };

    let bytes = t.encode();
    let back = Telemetry::parse(&bytes).expect("往返应成功");
    assert_eq!(back, t, "编解码往返必须无损");

    let d1 = derived::derive(&t);
    let d2 = derived::derive(&back);
    assert_eq!(d1, d2, "往返后派生量必须一致");
}

#[test]
fn 速度常量的换算关系正确() {
    // 100 km/h ≈ 27.78 m/s
    assert!((100.0 / fh6_core::MPS_TO_KPH - 27.777).abs() < 0.01);
    assert!((fh6_core::STANDARD_GRAVITY - 9.80665).abs() < 1e-5);
}
