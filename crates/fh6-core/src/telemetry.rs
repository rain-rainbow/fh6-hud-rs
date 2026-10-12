//! 数据包解析：从 324 字节的 UDP 载荷读出 [`Telemetry`]。
//!
//! 解析全部显式按小端读取，不使用 `unsafe`、不做未对齐指针转型。
//! 所有偏移量集中在 [`crate::pack`]，本模块只负责「按偏移取字节并解释类型」。

use crate::pack::*;
use crate::raw::RawPacket;

/// 单个车轮的四个常用量。
///
/// 之所以把「每轮一组」的字段从数组拆成具名字段，是因为 HUD 渲染时按轮取值
/// 的频率极高（每帧多次），具名字段比 `arr[idx]` 更不容易写错下标。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Wheel {
    /// 轮胎滑移率（倒车打滑为负）。
    pub slip_ratio: f32,
    /// 轮胎滑移角。
    pub slip_angle: f32,
    /// 综合滑移量 —— 漂移判定的核心指标。
    pub combined_slip: f32,
    /// 归一化悬挂行程（0..1，1 为完全压缩）。
    pub norm_susp_travel: f32,
    /// 轮胎温度（摄氏度）。
    pub tire_temp: f32,
    /// 车轮转速（rad/s）。
    pub wheel_rotation_speed: f32,
    /// 悬挂行程（米）。
    pub suspension_travel_meters: f32,
    /// 路面颠簸强度。
    pub surface_rumble: f32,
    /// 是否在震动带/路肩上（非 0 为真）。
    pub on_rumble_strip: bool,
    /// 是否在积水中（非 0 为真）。
    pub in_puddle: bool,
}

/// 解析失败的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// 数据包长度不足 324 字节，字段读不全。
    ///
    /// 注意：**长度大于 324 不算错**，多出来的尾部字节会被忽略。
    /// 官方声明 324，但社区实现同时接受 323，因此这里只拒绝 `< 324`。
    TooShort {
        /// 实际收到的字节数。
        actual: usize,
    },
}

impl core::fmt::Display for ParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ParseError::TooShort { actual } => write!(
                f,
                "数据包长度不足：收到 {actual} 字节，至少需要 {PACKET_SIZE} 字节"
            ),
        }
    }
}

impl std::error::Error for ParseError {}

/// 一帧完整的遥测数据（已解析成强类型，可直接给 HUD 用）。
///
/// 这是「原始值」层：不做单位换算、不做滤波、不做业务判断。
/// 单位换算与派生量见 [`crate::derived`]。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Telemetry {
    /// 是否处于比赛中。false 时游戏不会发包，但收到的包也可能是 0。
    pub is_race_on: bool,
    /// 原始毫秒时间戳（会回绕，比较时请用 `wrapping_sub`）。
    pub timestamp_ms: u32,

    /// 发动机最大转速。
    pub engine_max_rpm: f32,
    /// 发动机怠速转速。
    pub engine_idle_rpm: f32,
    /// 当前转速。
    pub current_engine_rpm: f32,

    /// 世界坐标系加速度（m/s²）。Y 为上方向。
    pub acceleration: [f32; 3],
    /// 世界坐标系速度（m/s）。Y 为上方向。
    pub velocity: [f32; 3],
    /// 世界坐标系角速度（rad/s）。
    pub angular_velocity: [f32; 3],
    /// 姿态角（弧度）：[yaw, pitch, roll]。
    pub orientation: [f32; 3],

    // ---- 四个车轮 ----
    /// 前左轮。
    pub wheel_fl: Wheel,
    /// 前右轮。
    pub wheel_fr: Wheel,
    /// 后左轮。漂移 HUD 最关注。
    pub wheel_rl: Wheel,
    /// 后右轮。漂移 HUD 最关注。
    pub wheel_rr: Wheel,

    // ---- 车辆静态信息 ----
    /// 车辆唯一编号（用于查车名表，官方不提供映射）。
    pub car_ordinal: i32,
    /// 车辆等级。
    pub car_class: i32,
    /// 性能指数。
    pub car_performance_index: i32,
    /// 传动类型：0=FWD, 1=RWD, 2=AWD。
    pub drivetrain_type: i32,
    /// 气缸数。
    pub num_cylinders: i32,
    /// 车辆分组。FH6 新增语义。
    pub car_group: u32,

    /// 撞碎物速度差。FH6 新增语义。
    pub smashable_vel_diff: f32,
    /// 撞碎物质量。FH6 新增语义。
    pub smashable_mass: f32,

    // ---- 位姿与动力 ----
    /// 世界坐标位置（米）：[X, Y(高度), Z]。
    pub position: [f32; 3],
    /// **速度，单位 m/s**（不是 km/h）。
    pub speed_mps: f32,
    /// 功率（瓦）。
    pub power_w: f32,
    /// 扭矩（牛·米）。
    pub torque_nm: f32,
    /// 涡轮增压值。
    pub boost: f32,
    /// 油量 0.0..1.0。
    pub fuel: f32,
    /// 已行驶距离（米）。
    pub distance_traveled_m: f32,

    // ---- 计时 ----
    /// 最佳圈速（秒）。
    pub best_lap_s: f32,
    /// 上一圈用时（秒）。
    pub last_lap_s: f32,
    /// 当前圈已用时（秒）。
    pub current_lap_s: f32,
    /// 本场比赛总用时（秒）。
    pub current_race_time_s: f32,

    // ---- 比赛状态与输入 ----
    /// 圈数。
    pub lap_number: u16,
    /// 名次（1 起）。
    pub race_position: u8,
    /// 油门原始值 0..255。
    pub accel_raw: u8,
    /// 刹车原始值 0..255。
    pub brake_raw: u8,
    /// 离合原始值 0..255。
    pub clutch_raw: u8,
    /// 手刹原始值 0..255。
    pub handbrake_raw: u8,
    /// 挡位：**0 表示倒挡 (R)**。
    pub gear_raw: u8,
    /// 方向盘原始值 -127..127（负为左）。
    pub steer_raw: i8,
    /// 归一化行车线。
    pub normalized_driving_line: i8,
    /// 归一化 AI 刹车差。
    pub normalized_ai_brake_difference: i8,
}

impl Telemetry {
    /// 从 UDP 收到的字节切片解析出一帧遥测。
    ///
    /// * 长度 `>= 324` → 解析成功，尾部多余字节忽略；
    /// * 长度 `< 324` → 返回 [`ParseError::TooShort`]。
    ///
    /// 浮点数出现 NaN / 无穷时**不报错**，原样保留，由派生层决定如何处理
    /// ——因为游戏在某些瞬态（如传送、重置）下确实可能发出异常值，
    /// 直接拒绝整包会让 HUD 闪断。
    pub fn parse(buf: &[u8]) -> Result<Self, ParseError> {
        if buf.len() < PACKET_SIZE {
            return Err(ParseError::TooShort { actual: buf.len() });
        }

        // 显式指定 324 长度后取数组引用，让后续 helper 的越界检查
        // 由类型系统在编译期消除（配合内联后基本无运行时开销）。
        let mut p = [0u8; PACKET_SIZE];
        p.copy_from_slice(&buf[..PACKET_SIZE]);
        Ok(Self::from_array(&p))
    }

    /// 从定长数组解析。`PACKET_SIZE` 保证所有偏移量都在界内。
    pub fn from_array(p: &[u8; PACKET_SIZE]) -> Self {
        Self {
            is_race_on: rd_i32(p, IS_RACE_ON) != 0,
            timestamp_ms: rd_u32(p, TIMESTAMP_MS),

            engine_max_rpm: rd_f32(p, ENGINE_MAX_RPM),
            engine_idle_rpm: rd_f32(p, ENGINE_IDLE_RPM),
            current_engine_rpm: rd_f32(p, CURRENT_ENGINE_RPM),

            acceleration: [
                rd_f32(p, ACCELERATION),
                rd_f32(p, ACCELERATION + 4),
                rd_f32(p, ACCELERATION + 8),
            ],
            velocity: [
                rd_f32(p, VELOCITY),
                rd_f32(p, VELOCITY + 4),
                rd_f32(p, VELOCITY + 8),
            ],
            angular_velocity: [
                rd_f32(p, ANGULAR_VELOCITY),
                rd_f32(p, ANGULAR_VELOCITY + 4),
                rd_f32(p, ANGULAR_VELOCITY + 8),
            ],
            orientation: [rd_f32(p, YAW), rd_f32(p, PITCH), rd_f32(p, ROLL)],

            wheel_fl: Self::read_wheel(p, WHEEL_FL),
            wheel_fr: Self::read_wheel(p, WHEEL_FR),
            wheel_rl: Self::read_wheel(p, WHEEL_RL),
            wheel_rr: Self::read_wheel(p, WHEEL_RR),

            car_ordinal: rd_i32(p, CAR_ORDINAL),
            car_class: rd_i32(p, CAR_CLASS),
            car_performance_index: rd_i32(p, CAR_PERFORMANCE_INDEX),
            drivetrain_type: rd_i32(p, DRIVETRAIN_TYPE),
            num_cylinders: rd_i32(p, NUM_CYLINDERS),
            car_group: rd_u32(p, CAR_GROUP),

            smashable_vel_diff: rd_f32(p, SMASHABLE_VEL_DIFF),
            smashable_mass: rd_f32(p, SMASHABLE_MASS),

            position: [
                rd_f32(p, POSITION),
                rd_f32(p, POSITION + 4),
                rd_f32(p, POSITION + 8),
            ],
            speed_mps: rd_f32(p, SPEED),
            power_w: rd_f32(p, POWER),
            torque_nm: rd_f32(p, TORQUE),
            boost: rd_f32(p, BOOST),
            fuel: rd_f32(p, FUEL),
            distance_traveled_m: rd_f32(p, DISTANCE_TRAVELED),

            best_lap_s: rd_f32(p, BEST_LAP),
            last_lap_s: rd_f32(p, LAST_LAP),
            current_lap_s: rd_f32(p, CURRENT_LAP),
            current_race_time_s: rd_f32(p, CURRENT_RACE_TIME),

            lap_number: rd_u16(p, LAP_NUMBER),
            race_position: rd_u8(p, RACE_POSITION),
            accel_raw: rd_u8(p, ACCEL),
            brake_raw: rd_u8(p, BRAKE),
            clutch_raw: rd_u8(p, CLUTCH),
            handbrake_raw: rd_u8(p, HANDBRAKE),
            gear_raw: rd_u8(p, GEAR),
            steer_raw: rd_i8(p, STEER),
            normalized_driving_line: rd_i8(p, NORMALIZED_DRIVING_LINE),
            normalized_ai_brake_difference: rd_i8(p, NORMALIZED_AI_BRAKE_DIFFERENCE),
        }
    }

    /// 读取第 `wheel` 个车轮的一组字段。
    #[inline]
    fn read_wheel(p: &[u8; PACKET_SIZE], wheel: usize) -> Wheel {
        Wheel {
            slip_ratio: rd_f32(p, RawPacket::wheel_offset(TIRE_SLIP_RATIO, wheel)),
            slip_angle: rd_f32(p, RawPacket::wheel_offset(TIRE_SLIP_ANGLE, wheel)),
            combined_slip: rd_f32(p, RawPacket::wheel_offset(TIRE_COMBINED_SLIP, wheel)),
            norm_susp_travel: rd_f32(p, RawPacket::wheel_offset(NORM_SUSP_TRAVEL, wheel)),
            tire_temp: rd_f32(p, RawPacket::wheel_offset(TIRE_TEMP, wheel)),
            wheel_rotation_speed: rd_f32(p, RawPacket::wheel_offset(WHEEL_ROTATION_SPEED, wheel)),
            suspension_travel_meters: rd_f32(
                p,
                RawPacket::wheel_offset(SUSPENSION_TRAVEL_METERS, wheel),
            ),
            surface_rumble: rd_f32(p, RawPacket::wheel_offset(SURFACE_RUMBLE, wheel)),
            on_rumble_strip: rd_i32(p, RawPacket::wheel_offset(WHEEL_ON_RUMBLE_STRIP, wheel)) != 0,
            in_puddle: rd_f32(p, RawPacket::wheel_offset(WHEEL_IN_PUDDLE, wheel)) != 0.0,
        }
    }

    /// 后轴平均综合滑移量 —— 漂移强度的主指标。
    ///
    /// 取绝对值后平均：倒车打滑时滑移率为负，但「打滑程度」应该看幅值。
    #[inline]
    pub fn rear_combined_slip(&self) -> f32 {
        (self.wheel_rl.combined_slip.abs() + self.wheel_rr.combined_slip.abs()) * 0.5
    }

    /// 速度的 km/h 表示（原始值是 m/s）。
    #[inline]
    pub fn speed_kph(&self) -> f32 {
        self.speed_mps * MPS_TO_KPH
    }

    /// 返回挡位显示文本：0 → `"R"`（倒挡），其余为数字。
    pub fn gear_label(&self) -> String {
        if self.gear_raw == 0 {
            "R".to_string()
        } else {
            self.gear_raw.to_string()
        }
    }

    /// 按文档布局把本帧**写回** 324 字节。
    ///
    /// 生产代码不用它，它的用途是测试：
    /// 构造「已知数值 → 字节 → 解析 → 应还原同样数值」的往返测试，
    /// 这样任何偏移量写错都会被测试抓到。
    ///
    /// offset 323 的保留字节写 0。
    pub fn encode(&self) -> [u8; PACKET_SIZE] {
        let mut p = [0u8; PACKET_SIZE];
        wr_i32(&mut p, IS_RACE_ON, if self.is_race_on { 1 } else { 0 });
        wr_u32(&mut p, TIMESTAMP_MS, self.timestamp_ms);

        wr_f32(&mut p, ENGINE_MAX_RPM, self.engine_max_rpm);
        wr_f32(&mut p, ENGINE_IDLE_RPM, self.engine_idle_rpm);
        wr_f32(&mut p, CURRENT_ENGINE_RPM, self.current_engine_rpm);

        for i in 0..3 {
            wr_f32(&mut p, ACCELERATION + i * 4, self.acceleration[i]);
            wr_f32(&mut p, VELOCITY + i * 4, self.velocity[i]);
            wr_f32(&mut p, ANGULAR_VELOCITY + i * 4, self.angular_velocity[i]);
            wr_f32(&mut p, POSITION + i * 4, self.position[i]);
        }
        wr_f32(&mut p, YAW, self.orientation[0]);
        wr_f32(&mut p, PITCH, self.orientation[1]);
        wr_f32(&mut p, ROLL, self.orientation[2]);

        for (idx, w) in [
            (WHEEL_FL, self.wheel_fl),
            (WHEEL_FR, self.wheel_fr),
            (WHEEL_RL, self.wheel_rl),
            (WHEEL_RR, self.wheel_rr),
        ] {
            wr_f32(
                &mut p,
                RawPacket::wheel_offset(TIRE_SLIP_RATIO, idx),
                w.slip_ratio,
            );
            wr_f32(
                &mut p,
                RawPacket::wheel_offset(TIRE_SLIP_ANGLE, idx),
                w.slip_angle,
            );
            wr_f32(
                &mut p,
                RawPacket::wheel_offset(TIRE_COMBINED_SLIP, idx),
                w.combined_slip,
            );
            wr_f32(
                &mut p,
                RawPacket::wheel_offset(NORM_SUSP_TRAVEL, idx),
                w.norm_susp_travel,
            );
            wr_f32(&mut p, RawPacket::wheel_offset(TIRE_TEMP, idx), w.tire_temp);
            wr_f32(
                &mut p,
                RawPacket::wheel_offset(WHEEL_ROTATION_SPEED, idx),
                w.wheel_rotation_speed,
            );
            wr_f32(
                &mut p,
                RawPacket::wheel_offset(SUSPENSION_TRAVEL_METERS, idx),
                w.suspension_travel_meters,
            );
            wr_f32(
                &mut p,
                RawPacket::wheel_offset(SURFACE_RUMBLE, idx),
                w.surface_rumble,
            );
            wr_i32(
                &mut p,
                RawPacket::wheel_offset(WHEEL_ON_RUMBLE_STRIP, idx),
                if w.on_rumble_strip { 1 } else { 0 },
            );
            wr_f32(
                &mut p,
                RawPacket::wheel_offset(WHEEL_IN_PUDDLE, idx),
                if w.in_puddle { 1.0 } else { 0.0 },
            );
        }

        wr_i32(&mut p, CAR_ORDINAL, self.car_ordinal);
        wr_i32(&mut p, CAR_CLASS, self.car_class);
        wr_i32(&mut p, CAR_PERFORMANCE_INDEX, self.car_performance_index);
        wr_i32(&mut p, DRIVETRAIN_TYPE, self.drivetrain_type);
        wr_i32(&mut p, NUM_CYLINDERS, self.num_cylinders);
        wr_u32(&mut p, CAR_GROUP, self.car_group);

        wr_f32(&mut p, SMASHABLE_VEL_DIFF, self.smashable_vel_diff);
        wr_f32(&mut p, SMASHABLE_MASS, self.smashable_mass);

        wr_f32(&mut p, SPEED, self.speed_mps);
        wr_f32(&mut p, POWER, self.power_w);
        wr_f32(&mut p, TORQUE, self.torque_nm);
        wr_f32(&mut p, BOOST, self.boost);
        wr_f32(&mut p, FUEL, self.fuel);
        wr_f32(&mut p, DISTANCE_TRAVELED, self.distance_traveled_m);

        wr_f32(&mut p, BEST_LAP, self.best_lap_s);
        wr_f32(&mut p, LAST_LAP, self.last_lap_s);
        wr_f32(&mut p, CURRENT_LAP, self.current_lap_s);
        wr_f32(&mut p, CURRENT_RACE_TIME, self.current_race_time_s);

        wr_u16(&mut p, LAP_NUMBER, self.lap_number);
        wr_u8(&mut p, RACE_POSITION, self.race_position);
        wr_u8(&mut p, ACCEL, self.accel_raw);
        wr_u8(&mut p, BRAKE, self.brake_raw);
        wr_u8(&mut p, CLUTCH, self.clutch_raw);
        wr_u8(&mut p, HANDBRAKE, self.handbrake_raw);
        wr_u8(&mut p, GEAR, self.gear_raw);
        wr_i8(&mut p, STEER, self.steer_raw);
        wr_i8(
            &mut p,
            NORMALIZED_DRIVING_LINE,
            self.normalized_driving_line,
        );
        wr_i8(
            &mut p,
            NORMALIZED_AI_BRAKE_DIFFERENCE,
            self.normalized_ai_brake_difference,
        );

        // offset 323 保持为 0 —— 官方未解释的保留填充字节。
        debug_assert_eq!(p[323], 0);
        p
    }
}

// ============================================================================
// 底层读写 helper
// ============================================================================
//
// 全部用 `from_le_bytes` / `to_le_bytes`：显式小端、无 unsafe、无对齐要求。
// 这几个函数都会在 `PACKET_SIZE` 定长数组上被内联，越界检查可被优化掉。

#[inline]
fn rd_u32(b: &[u8; PACKET_SIZE], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

#[inline]
fn rd_i32(b: &[u8; PACKET_SIZE], o: usize) -> i32 {
    i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

#[inline]
fn rd_f32(b: &[u8; PACKET_SIZE], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

#[inline]
fn rd_u16(b: &[u8; PACKET_SIZE], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

#[inline]
fn rd_u8(b: &[u8; PACKET_SIZE], o: usize) -> u8 {
    b[o]
}

#[inline]
fn rd_i8(b: &[u8; PACKET_SIZE], o: usize) -> i8 {
    b[o] as i8
}

#[inline]
fn wr_u32(b: &mut [u8; PACKET_SIZE], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

#[inline]
fn wr_i32(b: &mut [u8; PACKET_SIZE], o: usize, v: i32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

#[inline]
fn wr_f32(b: &mut [u8; PACKET_SIZE], o: usize, v: f32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

#[inline]
fn wr_u16(b: &mut [u8; PACKET_SIZE], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}

#[inline]
fn wr_u8(b: &mut [u8; PACKET_SIZE], o: usize, v: u8) {
    b[o] = v;
}

#[inline]
fn wr_i8(b: &mut [u8; PACKET_SIZE], o: usize, v: i8) {
    b[o] = v as u8;
}
