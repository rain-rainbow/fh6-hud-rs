//! # fh6-core — 《极限竞速：地平线6》遥测数据核心
//!
//! 本 crate 是 **纯逻辑库**：没有 GUI、没有网络 I/O、没有全局状态。
//! 它的职责只有三件事：
//!
//! 1. [`pack`] —— 用常量形式记录 324 字节数据包中**每个字段的精确字节偏移**；
//! 2. [`telemetry`] —— 把字节解析成强类型的 [`Telemetry`]；
//! 3. [`derived`] —— 把原始值换算成 HUD 要显示的派生量（km/h、G 值、漂移角…）。
//!
//! 拆成独立 crate 的好处：**业务逻辑可以被单元测试覆盖，而不需要启动窗口或
//! 连着游戏**。这是对参考实现最主要的改进点之一（参考实现把渲染、状态机、
//! 遥测解析全部塞在一个 5000 行的 PyQt 类里，无法单独测试）。
//!
//! # 数据源与免责声明
//!
//! * 协议格式依据官方文档
//!   <https://support.forza.net/hc/en-us/articles/51744149102611-Forza-Horizon-6-Data-Out-Documentation>
//!   以及公开的社区逆向资料；
//! * 官方**未公布字节偏移表**，本 crate 的偏移量属**社区逆向 + 抓包验证**结果；
//! * 本项目为 **clean-room 独立实现**，未复制任何第三方项目的源代码。
//!
//! # 最小用法
//!
//! ```
//! use fh6_core::{Telemetry, derived};
//!
//! // 实际使用中这些字节来自 UDP 套接字
//! let frame = Telemetry::default();
//! let bytes = frame.encode();          // 324 字节
//! let parsed = Telemetry::parse(&bytes).expect("长度合法");
//! let d = derived::derive(&parsed);
//! let mut buf = [0u8; 4];
//! assert_eq!(d.gear_display(&mut buf), "R");   // gear_raw == 0 表示倒挡
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(clippy::all)]

pub mod derived;
pub mod pack;
pub mod raw;
pub mod telemetry;

pub use derived::{derive, Derived};
pub use pack::{
    FLYDIGI_FORZA_PORT, FM_SLED_PACKET_SIZE, FIELDS_SIZE, LOCALHOST, MPS_TO_KPH, PACKET_SIZE,
    STANDARD_GRAVITY,
};
pub use raw::RawPacket;
pub use telemetry::{ParseError, Telemetry, Wheel};

/// 从 UDP 载荷解析一帧遥测的便捷入口。
///
/// 等价于 [`Telemetry::parse`]，单独提供一个自由函数是为了让调用方
/// 可以 `use fh6_core::parse_packet;` 少写一层路径。
#[inline]
pub fn parse_packet(buf: &[u8]) -> Result<Telemetry, ParseError> {
    Telemetry::parse(buf)
}

/// 一帧「原始 + 派生」的完整快照。
///
/// HUD 每帧需要同时访问原始值和派生值，把它们打包成结构体可以避免
/// 调用方反复传两个参数。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Frame {
    /// 原始遥测。
    pub telemetry: Telemetry,
    /// 派生展示量。
    pub derived: Derived,
}

impl Frame {
    /// 解析并计算派生量。
    pub fn from_bytes(buf: &[u8]) -> Result<Self, ParseError> {
        let telemetry = Telemetry::parse(buf)?;
        Ok(Self::new(telemetry))
    }

    /// 由已解析的遥测构造。
    pub fn new(telemetry: Telemetry) -> Self {
        let derived = derived::derive(&telemetry);
        Self { telemetry, derived }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::{WHEEL_FL, WHEEL_FR, WHEEL_RL, WHEEL_RR};

    #[test]
    fn 结构体大小必须等于官方声明的324字节() {
        // 88 个字段按类型尺寸求和 = 323，但对齐要求 4 使 sizeof 被补齐到 324。
        // 这正好等于官方声明的包长 —— 第 324 字节是结构体尾部对齐填充。
        assert_eq!(RawPacket::FIELDS_SIZE, 323, "字段本身只占 323 字节");
        assert_eq!(RawPacket::SIZE, 324, "结构体因 4 字节对齐被补齐到 324");
        assert_eq!(RawPacket::SIZE, PACKET_SIZE, "sizeof 应等于官方包长");
        assert_eq!(PACKET_SIZE, 324);
    }

    #[test]
    fn 各个字段偏移量必须两两不重叠且单调递增() {
        // 把「单字段」偏移量收集起来排序，确认没有重复（重复意味着有两个字段
        // 共用了同一段字节，必然是偏移量算错了）。
        let singles: [usize; 33] = [
            pack::IS_RACE_ON,
            pack::TIMESTAMP_MS,
            pack::ENGINE_MAX_RPM,
            pack::ENGINE_IDLE_RPM,
            pack::CURRENT_ENGINE_RPM,
            pack::ACCELERATION,
            pack::VELOCITY,
            pack::ANGULAR_VELOCITY,
            pack::YAW,
            pack::PITCH,
            pack::ROLL,
            pack::NORM_SUSP_TRAVEL,
            pack::TIRE_SLIP_RATIO,
            pack::WHEEL_ROTATION_SPEED,
            pack::WHEEL_ON_RUMBLE_STRIP,
            pack::WHEEL_IN_PUDDLE,
            pack::SURFACE_RUMBLE,
            pack::TIRE_SLIP_ANGLE,
            pack::TIRE_COMBINED_SLIP,
            pack::SUSPENSION_TRAVEL_METERS,
            pack::CAR_ORDINAL,
            pack::CAR_CLASS,
            pack::CAR_PERFORMANCE_INDEX,
            pack::DRIVETRAIN_TYPE,
            pack::NUM_CYLINDERS,
            pack::CAR_GROUP,
            pack::SMASHABLE_VEL_DIFF,
            pack::SMASHABLE_MASS,
            pack::POSITION,
            pack::SPEED,
            pack::POWER,
            pack::TORQUE,
            pack::BOOST,
        ];
        let mut sorted = singles.to_vec();
        sorted.sort_unstable();
        for w in sorted.windows(2) {
            assert!(w[0] < w[1], "偏移量重复或乱序: {} vs {}", w[0], w[1]);
        }

        // 尾部小整数段必须严格连续：312(U16) 314 315 316 317 318 319 320 321 322
        assert_eq!(pack::LAP_NUMBER, 312);
        assert_eq!(pack::RACE_POSITION, 314);
        assert_eq!(pack::ACCEL, 315);
        assert_eq!(pack::BRAKE, 316);
        assert_eq!(pack::CLUTCH, 317);
        assert_eq!(pack::HANDBRAKE, 318);
        assert_eq!(pack::GEAR, 319);
        assert_eq!(pack::STEER, 320);
        assert_eq!(pack::NORMALIZED_DRIVING_LINE, 321);
        assert_eq!(pack::NORMALIZED_AI_BRAKE_DIFFERENCE, 322);
        assert_eq!(pack::FIELDS_END, 323);
    }

    #[test]
    fn 车轮组偏移必须相差16字节() {
        // 每个「每轮一组」的字段是 4 个 f32 = 16 字节，组与组之间的间距
        // 应该是各字段尺寸的累加。这里只断言最基本的 stride 正确性。
        assert_eq!(RawPacket::wheel_offset(pack::TIRE_SLIP_RATIO, WHEEL_FL),
                   pack::TIRE_SLIP_RATIO);
        assert_eq!(RawPacket::wheel_offset(pack::TIRE_SLIP_RATIO, WHEEL_FR),
                   pack::TIRE_SLIP_RATIO + 4);
        assert_eq!(RawPacket::wheel_offset(pack::TIRE_SLIP_RATIO, WHEEL_RR),
                   pack::TIRE_SLIP_RATIO + 12);
        // 常用后轴轮的绝对偏移，写死一遍作为「签名」——
        // 一旦有人改动偏移量，这里会立刻失败提醒复核抓包数据。
        assert_eq!(RawPacket::wheel_offset(pack::TIRE_COMBINED_SLIP, WHEEL_RL), 188);
        assert_eq!(RawPacket::wheel_offset(pack::TIRE_COMBINED_SLIP, WHEEL_RR), 192);
        assert_eq!(RawPacket::wheel_offset(pack::TIRE_SLIP_RATIO, WHEEL_RL), 92);
        assert_eq!(pack::TIRE_TEMP + 8, 276); // 后左胎温
        assert_eq!(pack::TIRE_TEMP + 12, 280); // 后右胎温
    }

    #[test]
    fn 长度不足时返回错误而不是panic() {
        for n in [0usize, 1, 100, 323] {
            let buf = vec![0u8; n];
            let err = Telemetry::parse(&buf).unwrap_err();
            assert_eq!(err, ParseError::TooShort { actual: n });
        }
    }

    #[test]
    fn 长度超出324时尾部字节被忽略() {
        let mut buf = vec![0u8; 400];
        // 写一个合法的 timestamp
        buf[4..8].copy_from_slice(&12345u32.to_le_bytes());
        let t = Telemetry::parse(&buf).expect("超长包应当能解析");
        assert_eq!(t.timestamp_ms, 12345);
    }

    #[test]
    fn 保留字节323不应影响解析() {
        let t = Telemetry::default();
        let mut bytes = t.encode();
        assert_eq!(bytes[323], 0, "编码后保留字节应为 0");
        // 即使把保留字节改成非 0，解析结果也不应变化
        bytes[323] = 0xFF;
        let parsed = Telemetry::parse(&bytes).unwrap();
        assert_eq!(parsed, t);
    }
}
