//! 与数据包一一对应的 `#[repr(C)]` 原始结构体定义。
//!
//! # 这个结构体为什么存在
//!
//! 解析本身是**按字节偏移逐字段读取**（见 [`crate::telemetry`]），并不依赖
//! 这个结构体。它存在的价值是**作为布局的独立交叉验证**：
//!
//! * 单元测试断言 `size_of::<RawPacket>() == 323`；
//! * 如果 [`crate::pack`] 里某个偏移量算错了，结构体字段顺序与偏移常量就会
//!   产生矛盾，测试会立刻暴露。
//!
//! # 为什么不用它直接做「零拷贝转型」
//!
//! 理论上可以 `unsafe` 把 `&[u8;324]` 转成 `&RawPacket`，但有两个坑：
//!
//! 1. **对齐**：UDP 收到的缓冲区不保证 4 字节对齐，`f32` 未对齐读取在
//!    某些架构上是 UB。x86 虽然容忍，但没有必要冒这个险。
//! 2. **字节序**：必须是小端。x86 是小端所以「恰好能用」，但这是隐式假设。
//!
//! 显式逐字段 `from_le_bytes` 的代价极低（60 Hz、324 字节，CPU 开销可忽略），
//! 换来的是**无 unsafe、无对齐假设、字节序显式**。这符合本项目「降低资源
//! 占用靠轻量架构而不是靠投机取巧」的取舍。

use crate::pack::*;

/// FH6「数据输出」数据包的原始内存镜像（小端，共 323 字节有效数据 + 1 字节对齐填充）。
///
/// 字段顺序**严格**按官方文档给出的类型与顺序排列。
///
/// # `size_of` 为什么是 324 而不是 323 —— 一个被本测试证实的结论
///
/// 88 个字段按类型尺寸求和是 **323** 字节，但 `size_of::<RawPacket>()` 实测为
/// **324**。原因不是"多了一个字段"，而是**编译器的结构体尾部对齐填充**：
/// 结构体内含 `f32` / `i32` / `u32`（对齐要求 4 字节），因此整个结构体的大小
/// 必须被补齐到 4 的倍数 —— 323 向上取整即 324。
///
/// 这恰好解释了官方文档那句没人能解释的
/// `Total packet size: 324 bytes.`：
///
/// > **第 324 字节就是 C 结构体的对齐填充，不是任何字段。**
///
/// 官方文档只描述字段类型与顺序、不给偏移表，很可能就是直接从这份 C 结构体
/// 生成文档的，于是把 `sizeof` 的结果（324）写成了包长。
///
/// 这也与实测相符：抓包中 offset 323 恒为 0，且"字段和 323 + 填充 1 = 324"
/// 完全自洽。因此在实现上：
///
/// * 结构体保留 4 字节对齐带来的尾部填充，`size_of` == 324 == [`PACKET_SIZE`]；
/// * 但**字段总数仍是 88 个**，最后一个字段结束于 offset 322；
/// * 解析时忽略第 324 字节，不对它做任何语义假设。
///
/// 命名上 `_fl/_fr/_rl/_rr` 四个后缀表示四个车轮：
/// `fl`=前左、`fr`=前右、`rl`=后左、`rr`=后右。
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct RawPacket {
    /// `S32` — 是否处于比赛中（1=比赛，0=菜单/暂停）。偏移 0。
    pub is_race_on: i32,
    /// `U32` — 毫秒时间戳，会回绕。偏移 4。
    pub timestamp_ms: u32,

    /// `F32` — 发动机最大转速。偏移 8。
    pub engine_max_rpm: f32,
    /// `F32` — 发动机怠速转速。偏移 12。
    pub engine_idle_rpm: f32,
    /// `F32` — 当前转速。偏移 16。
    pub current_engine_rpm: f32,

    /// `F32` — 世界坐标系加速度 X（m/s²）。偏移 20。
    pub acceleration_x: f32,
    /// `F32` — 世界坐标系加速度 Y（m/s²，上方向）。偏移 24。
    pub acceleration_y: f32,
    /// `F32` — 世界坐标系加速度 Z（m/s²）。偏移 28。
    pub acceleration_z: f32,

    /// `F32` — 世界坐标系速度 X（m/s）。偏移 32。
    pub velocity_x: f32,
    /// `F32` — 世界坐标系速度 Y（m/s，上方向）。偏移 36。
    pub velocity_y: f32,
    /// `F32` — 世界坐标系速度 Z（m/s）。偏移 40。
    pub velocity_z: f32,

    /// `F32` — 角速度 X（rad/s）。偏移 44。
    pub angular_velocity_x: f32,
    /// `F32` — 角速度 Y（rad/s）。偏移 48。
    pub angular_velocity_y: f32,
    /// `F32` — 角速度 Z（rad/s）。偏移 52。
    pub angular_velocity_z: f32,

    /// `F32` — 偏航角（弧度）。偏移 56。
    pub yaw: f32,
    /// `F32` — 俯仰角（弧度）。偏移 60。
    pub pitch: f32,
    /// `F32` — 横滚角（弧度）。偏移 64。
    pub roll: f32,

    /// `F32[4]` — 归一化悬挂行程，顺序 FL/FR/RL/RR。偏移 68。
    pub norm_susp_travel: [f32; 4],
    /// `F32[4]` — 轮胎滑移率。偏移 84。倒车打滑时为负值。
    pub tire_slip_ratio: [f32; 4],
    /// `F32[4]` — 车轮转速（rad/s）。偏移 100。
    pub wheel_rotation_speed: [f32; 4],
    /// `S32[4]` — 是否在震动带/路肩上。偏移 116。
    pub wheel_on_rumble_strip: [i32; 4],
    /// `F32[4]` — 是否在积水中。偏移 132。
    pub wheel_in_puddle: [f32; 4],
    /// `F32[4]` — 路面颠簸强度。偏移 148。
    pub surface_rumble: [f32; 4],
    /// `F32[4]` — 轮胎滑移角。偏移 164。
    pub tire_slip_angle: [f32; 4],
    /// `F32[4]` — **综合滑移量**，漂移 HUD 的核心指标之一。偏移 180。
    pub tire_combined_slip: [f32; 4],
    /// `F32[4]` — 悬挂行程（米）。偏移 196。
    pub suspension_travel_meters: [f32; 4],

    /// `S32` — 车辆唯一编号。偏移 212。
    pub car_ordinal: i32,
    /// `S32` — 车辆等级。偏移 216。
    pub car_class: i32,
    /// `S32` — 性能指数。偏移 220。
    pub car_performance_index: i32,
    /// `S32` — 传动类型（0=FWD, 1=RWD, 2=AWD）。偏移 224。
    pub drivetrain_type: i32,
    /// `S32` — 气缸数。偏移 228。
    pub num_cylinders: i32,

    /// `U32` — 车辆分组。偏移 232。⚠️ FH6 新增语义，FH5 里这 4 字节含义未知。
    pub car_group: u32,
    /// `F32` — 撞碎物速度差。偏移 236。⚠️ FH6 新增语义。
    pub smashable_vel_diff: f32,
    /// `F32` — 撞碎物质量。偏移 240。⚠️ FH6 新增语义。
    pub smashable_mass: f32,

    /// `F32` — 世界坐标位置 X（米）。偏移 244。
    pub position_x: f32,
    /// `F32` — 世界坐标位置 Y（米，高度）。偏移 248。
    pub position_y: f32,
    /// `F32` — 世界坐标位置 Z（米）。偏移 252。
    pub position_z: f32,

    /// `F32` — **速度，单位 m/s**（不是 km/h！）。偏移 256。
    pub speed: f32,
    /// `F32` — 功率（瓦）。偏移 260。
    pub power: f32,
    /// `F32` — 扭矩（牛·米）。偏移 264。
    pub torque: f32,
    /// `F32[4]` — 轮胎温度（摄氏度）。偏移 268。
    pub tire_temp: [f32; 4],
    /// `F32` — 涡轮增压值。偏移 284。
    pub boost: f32,
    /// `F32` — 油量（0.0..1.0）。偏移 288。
    pub fuel: f32,
    /// `F32` — 已行驶距离（米）。偏移 292。
    pub distance_traveled: f32,

    /// `F32` — 最佳圈速（秒）。偏移 296。
    pub best_lap: f32,
    /// `F32` — 上一圈用时（秒）。偏移 300。
    pub last_lap: f32,
    /// `F32` — 当前圈已用时（秒）。偏移 304。
    pub current_lap: f32,
    /// `F32` — 本次比赛总用时（秒）。偏移 308。
    pub current_race_time: f32,

    /// `U16` — 圈数。偏移 312。
    pub lap_number: u16,
    /// `U8` — 名次。偏移 314。
    pub race_position: u8,
    /// `U8` — 油门 0..255。偏移 315。
    pub accel: u8,
    /// `U8` — 刹车 0..255。偏移 316。
    pub brake: u8,
    /// `U8` — 离合 0..255。偏移 317。
    pub clutch: u8,
    /// `U8` — 手刹 0..255。偏移 318。
    pub handbrake: u8,
    /// `U8` — 挡位，**0 表示倒挡**。偏移 319。
    pub gear: u8,
    /// `S8` — 方向盘 **有符号** -127..127。偏移 320。
    pub steer: i8,
    /// `S8` — 归一化行车线。偏移 321。
    pub normalized_driving_line: i8,
    /// `S8` — 归一化 AI 刹车差。偏移 322。
    pub normalized_ai_brake_difference: i8,
}

impl RawPacket {
    /// `size_of::<RawPacket>()`，等于 324（323 字节字段 + 1 字节尾部对齐填充）。
    ///
    /// 与 [`PACKET_SIZE`] 相等并非巧合：**官方声明的包长很可能就是这个值**。
    pub const SIZE: usize = core::mem::size_of::<Self>();

    /// 88 个字段实际占用的字节数（323），不含尾部对齐填充。
    pub const FIELDS_SIZE: usize = crate::pack::FIELDS_SIZE;

    /// 返回某组「4 个车轮」字段中指定车轮的偏移量。
    ///
    /// `group_base` 传入 [`TIRE_COMBINED_SLIP`] 这类组起始常量，
    /// `wheel` 传入 [`WHEEL_RL`] 这类车轮索引常量。
    #[inline]
    pub const fn wheel_offset(group_base: usize, wheel: usize) -> usize {
        group_base + wheel * WHEEL_STRIDE
    }
}
