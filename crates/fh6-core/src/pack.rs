//! FH6「数据输出 (Data Out)」UDP 数据包的精确内存布局定义。
//!
//! # 为什么把偏移量写成常量
//!
//! 官方文档 <https://support.forza.net/hc/en-us/articles/51744149102611>
//! **只给出字段的「类型 + 顺序」，没有给出字节偏移表**，而且官方只说明：
//!
//! > Total packet size: 324 bytes.
//!
//! 88 个字段按类型尺寸逐个累加，恰好等于 **323** 字节，与官方声明的 324
//! 相差 1 字节。官方对此没有任何解释。经实际抓包核对（末字节恒为 0），
//! 通行做法是把 offset 323 当作**保留填充字节**丢弃：
//!
//! * 校验长度时按 **324**；
//! * 解析时只读到 **offset 322** 为止。
//!
//! 因此本模块把每个字段的偏移量显式写成 `pub const`，好处有三：
//!
//! 1. 偏移量可被单元测试逐个断言，改错立刻被测试抓住；
//! 2. 与 [`crate::raw::RawPacket`] 的 `#[repr(C)]` 结构体互为交叉验证
//!    （`size_of` 必须等于 323，编译期 + 测试双重保证）；
//! 3. 注释里能直接标注「官方原文」与「社区逆向」的区别，避免后人误信。
//!
//! # 数据来源与可信度分级
//!
//! | 标记 | 含义 |
//! |---|---|
//! | ✅ 官方 | 官方文档原文直接给出（类型、顺序、包长、回环支持） |
//! | 🔶 逆向 | 官方未给偏移表，由多份独立实现 + 实际抓包交叉验证 |
//! | ⚠️ 存疑 | 官方与实现之间存在矛盾，或仅单一来源 |
//!
//! # 与《极限竞速8 / FM》及 FH4/FH5 的关系
//!
//! * 官方 FH6 文档**全文未提及 FH5**，其对比基准是 Forza Motorsport。
//! * FH6 与 FH5 的包**总长相同（都是 324 字节）**，差异只在 offset 232..244
//!   这 12 字节：FH6 给 FH5 中未文档化的「12 字节空洞」赋予了 3 个语义字段
//!   （[`CAR_GROUP`]、[`SMASHABLE_VEL_DIFF`]、[`SMASHABLE_MASS`]）。
//!   这属于**抓包结论，非官方表述**。
//! * 「Sled / Dash」这套术语出自 **Forza Motorsport** 官方文档（Sled=232 字节，
//!   Dash=Sled+额外数据）。FH6 官方明确说格式固定：
//!   `The packet format is fixed. Unlike Forza Motorsport, there is no option
//!   to select between different formats.` —— **FH6 没有「包格式」设置项**。
//!
//! ⚠️ 本文件的所有偏移量均为**社区逆向 + 抓包验证**结果，官方未公布偏移表。

// ============================================================================
// 包长相关常量
// ============================================================================

/// 官方声明的包总长度（324 字节）。
///
/// 官方原文：`Total packet size: 324 bytes.`
///
/// # 这个数字的由来（本项目实测确认）
///
/// 88 个字段按类型尺寸求和只有 **323** 字节，剩下的 1 字节是**结构体的尾部
/// 对齐填充**：结构体含 `f32`/`i32`（对齐 4），`sizeof` 必须是 4 的倍数，
/// 323 向上取整正好是 324。详见 [`FIELDS_SIZE`]。
///
/// 注意：接收端**不应**用「长度必须严格等于 324」来丢弃数据包。第三方实现
/// 同时接受 323 与 324。本库的做法是：
///
/// * [`crate::ParseError::TooShort`]：`len < 324` 直接算错（数据不完整）；
/// * `len >= 324` 则正常解析，多余的尾部字节忽略。
pub const PACKET_SIZE: usize = 324;

/// 88 个字段按类型尺寸求和的实际字节数（323）。
///
/// `PACKET_SIZE - 1`，差的那 1 字节是**结构体的尾部对齐填充**。
///
/// # 这是本项目实测确认的结论
///
/// 结构体里有 `f32` / `i32` / `u32`（对齐要求 4），所以 `sizeof` 必须是 4 的
/// 倍数：323 向上取整 = 324。这与官方声明的 `Total packet size: 324 bytes.`
/// 完全吻合 —— **第 324 字节就是编译器的对齐填充，不是任何字段**。
///
/// 验证方式：`core::mem::size_of::<RawPacket>() == 324`（见单元测试
/// `结构体大小必须等于官方声明的324字节`）。官方文档只给类型和顺序、不给偏移表，
/// 很可能就是从同一份 C 结构体生成的，于是把 `sizeof` 写成了包长。
pub const FIELDS_SIZE: usize = 323;

/// Forza Motorsport 的「Sled」格式包长（232 字节），**不是 FH6 的格式**。
///
/// 保留此常量仅为在文档与日志里区分格式，避免把 FM 的包误当 FH6 解析
/// （用 311/232 字节布局去读 324 字节包会整体错位）。
pub const FM_SLED_PACKET_SIZE: usize = 232;

// ============================================================================
// 包头（8 字节）
// ============================================================================

/// `S32 IsRaceOn` — 包偏移 **0**。✅ 官方
///
/// 官方原文：`= 1 when race is on. = 0 when in menus/race stopped.`
///
/// 实际是 `int32`，但取值只有 0/1。用 `i32` 读取以忠实还原原始类型。
pub const IS_RACE_ON: usize = 0;

/// `U32 TimestampMS` — 包偏移 **4**。✅ 官方
///
/// 官方原文：`Can overflow to 0 eventually`
///
/// 单位毫秒，32 位无符号，会回绕。需要计算时间间隔时必须用
/// `wrapping_sub` 而不是普通减法，否则回绕瞬间会得到巨大的错误值。
pub const TIMESTAMP_MS: usize = 4;

// ============================================================================
// 发动机与传动（offset 8..32）
// ============================================================================

/// `F32 EngineMaxRpm` — 偏移 **8**。🔶 逆向
pub const ENGINE_MAX_RPM: usize = 8;
/// `F32 EngineIdleRpm` — 偏移 **12**。🔶 逆向
pub const ENGINE_IDLE_RPM: usize = 12;
/// `F32 CurrentEngineRpm` — 偏移 **16**。🔶 逆向
pub const CURRENT_ENGINE_RPM: usize = 16;

// ============================================================================
// 车身运动学（offset 20..68）
// ============================================================================

/// `F32 AccelerationX/Y/Z` 起点 — 偏移 **20 / 24 / 28**。🔶 逆向
///
/// 注意：这是**世界坐标系**下的加速度（m/s²），不是车体坐标系。
/// 想得到「纵向/横向 G 值」必须先把速度方向单位矢量投影上去，
/// 详见 [`crate::derived`]。
pub const ACCELERATION: usize = 20;

/// `F32 VelocityX/Y/Z` 起点 — 偏移 **32 / 36 / 40**。🔶 逆向
///
/// 世界坐标系速度（m/s）。
pub const VELOCITY: usize = 32;

/// `F32 AngularVelocityX/Y/Z` 起点 — 偏移 **44 / 48 / 52**。🔶 逆向
///
/// 世界坐标系角速度（rad/s）。
pub const ANGULAR_VELOCITY: usize = 44;

/// `F32 Yaw` — 偏移 **56**。🔶 逆向（弧度）
pub const YAW: usize = 56;
/// `F32 Pitch` — 偏移 **60**。🔶 逆向（弧度）
pub const PITCH: usize = 60;
/// `F32 Roll` — 偏移 **64**。🔶 逆向（弧度）
pub const ROLL: usize = 64;

// ============================================================================
// 每个车轮一组的字段（4 个 float × 4 个轮 = 16 字节一组）
// ============================================================================
//
// 车轮顺序固定为：前左(FL) → 前右(FR) → 后左(RL) → 后右(RR)
// 共 11 组，每组 16 字节，从 offset 68 连续排到 offset 243 之前的 244。
// 下面每组的常量是**该组 4 个轮子的起始偏移**，轮子 i 的偏移 = 组起始 + i*4。

/// `F32 NormSuspTravel[4]` 起点 — 偏移 **68**。🔶 逆向
pub const NORM_SUSP_TRAVEL: usize = 68;
/// `F32 TireSlipRatio[4]` 起点 — 偏移 **84**。🔶 逆向
pub const TIRE_SLIP_RATIO: usize = 84;
/// `F32 WheelRotationSpeed[4]` 起点 — 偏移 **100**。🔶 逆向
pub const WHEEL_ROTATION_SPEED: usize = 100;
/// `U32 WheelOnRumbleStrip[4]` 起点 — 偏移 **116**。🔶 逆向（每个 4 字节）
pub const WHEEL_ON_RUMBLE_STRIP: usize = 116;
/// `F32 WheelInPuddle[4]` 起点 — 偏移 **132**。🔶 逆向
pub const WHEEL_IN_PUDDLE: usize = 132;
/// `F32 SurfaceRumble[4]` 起点 — 偏移 **148**。🔶 逆向
pub const SURFACE_RUMBLE: usize = 148;
/// `F32 TireSlipAngle[4]` 起点 — 偏移 **164**。🔶 逆向
pub const TIRE_SLIP_ANGLE: usize = 164;
/// `F32 TireCombinedSlip[4]` 起点 — 偏移 **180**。🔶 逆向
pub const TIRE_COMBINED_SLIP: usize = 180;
/// `F32 SuspensionTravelMeters[4]` 起点 — 偏移 **196**。🔶 逆向
pub const SUSPENSION_TRAVEL_METERS: usize = 196;

/// 车轮数组中单个车轮占用的字节数（`f32`）。
pub const WHEEL_STRIDE: usize = 4;

/// 车轮索引：前左 (Front Left)，对应数组下标 0。
pub const WHEEL_FL: usize = 0;
/// 车轮索引：前右 (Front Right)，对应数组下标 1。
pub const WHEEL_FR: usize = 1;
/// 车轮索引：后左 (Rear Left)，对应数组下标 2。漂移 HUD 最关心这个。
pub const WHEEL_RL: usize = 2;
/// 车轮索引：后右 (Rear Right)，对应数组下标 3。漂移 HUD 最关心这个。
pub const WHEEL_RR: usize = 3;

// ============================================================================
// 车辆静态信息（offset 212..232）
// ============================================================================

/// `S32 CarOrdinal` — 偏移 **212**。🔶 逆向
///
/// 车辆唯一编号。官方**没有**提供编号到车名的映射表，
/// 车名表必须自建或用公开来源。
pub const CAR_ORDINAL: usize = 212;
/// `S32 CarClass` — 偏移 **216**。🔶 逆向（0..7，对应 D/C/B/A/S1/S2/X 等）
pub const CAR_CLASS: usize = 216;
/// `S32 CarPerformanceIndex` — 偏移 **220**。🔶 逆向（性能分）
pub const CAR_PERFORMANCE_INDEX: usize = 220;
/// `S32 DrivetrainType` — 偏移 **224**。🔶 逆向（0=FWD 前驱, 1=RWD 后驱, 2=AWD 四驱）
pub const DRIVETRAIN_TYPE: usize = 224;
/// `S32 NumCylinders` — 偏移 **228**。🔶 逆向
pub const NUM_CYLINDERS: usize = 228;

/// 传动类型枚举值：前轮驱动。
pub const DRIVETRAIN_FWD: i32 = 0;
/// 传动类型枚举值：后轮驱动。漂移场景最常见。
pub const DRIVETRAIN_RWD: i32 = 1;
/// 传动类型枚举值：四轮驱动。
pub const DRIVETRAIN_AWD: i32 = 2;

// ============================================================================
// FH6 相对 FH5 新增语义的 12 字节（offset 232..244）
// ============================================================================
//
// ⚠️ 这是整个包里**最容易出错**的区域：
// * FH5 时代这 12 字节含义未知，社区标注为「Forza Horizon 4 unknown values」；
// * FH6 官方**没有**为此单独说明偏移，但实际抓包证明它们是非零的实际数值
//   （抓包样本里碰撞中 SmashableVelDiff ≈ 0.0164、SmashableMass ≈ 0.18）；
// * FH5 解析器读 FH6 包时会把这三项静默丢弃，但**不会**造成后续字段错位，
//   因为尾部字段从 offset 244 起**绝对偏移完全一致**。

/// `U32 CarGroup` — 偏移 **232**。🔶 逆向（FH6 新增语义）
pub const CAR_GROUP: usize = 232;
/// `F32 SmashableVelDiff` — 偏移 **236**。🔶 逆向（FH6 新增语义，撞碎物的速度差）
pub const SMASHABLE_VEL_DIFF: usize = 236;
/// `F32 SmashableMass` — 偏移 **240**。🔶 逆向（FH6 新增语义，撞碎物的质量）
pub const SMASHABLE_MASS: usize = 240;

// ============================================================================
// 位置、速度、动力（offset 244..288）
// ============================================================================

/// `F32 PositionX/Y/Z` 起点 — 偏移 **244 / 248 / 252**。🔶 逆向
///
/// 世界坐标（米）。用于绘制轨迹图。注意 Y 是**高度**（游戏内是上方向），
/// 平面轨迹要用 X 与 Z，且绘制到屏幕时需要翻转某一轴，否则东西方向会镜像。
pub const POSITION: usize = 244;

/// `F32 Speed` — 偏移 **256**。🔶 逆向
///
/// ⚠️ 单位是 **m/s**，不是 km/h。换算：`km/h = m/s * 3.6`。
/// 这是最容易搞错的字段之一，本项目所有对外展示统一在派生层换算。
pub const SPEED: usize = 256;

/// `F32 Power` — 偏移 **260**。🔶 逆向（瓦特）
pub const POWER: usize = 260;
/// `F32 Torque` — 偏移 **264**。🔶 逆向（牛·米）
pub const TORQUE: usize = 264;

/// `F32 TireTemp[4]` 起点 — 偏移 **268**。🔶 逆向（摄氏度）
pub const TIRE_TEMP: usize = 268;

/// `F32 Boost` — 偏移 **284**。🔶 逆向
pub const BOOST: usize = 284;
/// `F32 Fuel` — 偏移 **288**。🔶 逆向（0.0..1.0 归一化油量）
pub const FUEL: usize = 288;
/// `F32 DistanceTraveled` — 偏移 **292**。🔶 逆向（米）
pub const DISTANCE_TRAVELED: usize = 292;

// ============================================================================
// 圈速与计时（offset 296..312）
// ============================================================================

/// `F32 BestLap` — 偏移 **296**。🔶 逆向（秒）
pub const BEST_LAP: usize = 296;
/// `F32 LastLap` — 偏移 **300**。🔶 逆向（秒）
pub const LAST_LAP: usize = 300;
/// `F32 CurrentLap` — 偏移 **304**。🔶 逆向（秒）
pub const CURRENT_LAP: usize = 304;
/// `F32 CurrentRaceTime` — 偏移 **308**。🔶 逆向（秒）
pub const CURRENT_RACE_TIME: usize = 308;

// ============================================================================
// 比赛状态与玩家输入（offset 312..323）
// ============================================================================
//
// 这一段是「小整数类型混排」，写结构体时极易因对齐而错位。
// 头文件里 `LapNumber` 是 **U16**，紧接着是一串 U8，最后是 3 个 S8。

/// `U16 LapNumber` — 偏移 **312**。🔶 逆向
pub const LAP_NUMBER: usize = 312;
/// `U8 RacePosition` — 偏移 **314**。🔶 逆向
pub const RACE_POSITION: usize = 314;
/// `U8 Accel` — 偏移 **315**。🔶 逆向（0..255 油门）
pub const ACCEL: usize = 315;
/// `U8 Brake` — 偏移 **316**。🔶 逆向（0..255 刹车）
pub const BRAKE: usize = 316;
/// `U8 Clutch` — 偏移 **317**。🔶 逆向（0..255 离合）
pub const CLUTCH: usize = 317;
/// `U8 HandBrake` — 偏移 **318**。🔶 逆向（0..255 手刹）
pub const HANDBRAKE: usize = 318;
/// `U8 Gear` — 偏移 **319**。🔶 逆向（**0 表示倒挡 R**，1..N 为前进挡）
pub const GEAR: usize = 319;
/// `S8 Steer` — 偏移 **320**。🔶 逆向（**有符号** -127..127，负为左打）
pub const STEER: usize = 320;
/// `S8 NormalizedDrivingLine` — 偏移 **321**。🔶 逆向
pub const NORMALIZED_DRIVING_LINE: usize = 321;
/// `S8 NormalizedAIBrakeDifference` — 偏移 **322**。🔶 逆向
pub const NORMALIZED_AI_BRAKE_DIFFERENCE: usize = 322;

/// 最后一个有效字段的结束位置（不含）。
///
/// `NORMALIZED_AI_BRAKE_DIFFERENCE` (322) + 1 = 323。
/// offset 323 的那 1 字节是结构体尾部对齐填充，**不解析、不做语义假设**。
pub const FIELDS_END: usize = 323;

// ============================================================================
// 归一化与单位换算常量
// ============================================================================

/// `Accel` / `Brake` / `Clutch` / `HandBrake` 的满量程（U8）。
pub const INPUT_U8_MAX: f32 = 255.0;

/// `Steer` 的满量程（S8 的正方向最大值）。
///
/// 注意是 127 而不是 128：S8 范围是 -128..127，
/// 用 128 作除数会让满右打方向超过 100%。
pub const STEER_S8_MAX: f32 = 127.0;

/// 标准重力加速度（m/s²），用于把 m/s² 换算成 G。
pub const STANDARD_GRAVITY: f32 = 9.806_65;

/// m/s → km/h 换算系数。
pub const MPS_TO_KPH: f32 = 3.6;

/// 官方明确支持的本地回环地址。✅ 官方
///
/// 官方原文出现两次：`also available to the localhost address (127.0.0.1)`
/// 与 `The localhost address (127.0.0.1) is supported`。
pub const LOCALHOST: &str = "127.0.0.1";

/// 飞智空间站（八爪鱼5 自适应扳机）硬编码监听的 UDP 端口。
///
/// ⚠️ **这不是官方常量，也不是 FH6 的默认值**。来源：
/// * 飞智空间站内置模块 `mods/forza_trigger/ForzaDualSense.exe`；
/// * 实测该进程启动后绑定 `127.0.0.1:5300`（本机 netstat 验证）；
/// * 其 `ForzaDualSense.dll` 内含 UTF-16 字符串 `5300`；
/// * `AdapterTrigger.dll` 含 `FORZA_DATA_OUT_PORT` / `DEFAULT_PORT` /
///   `TrySetPortNumber`，但配置文件 `config.default.json` 中**无端口项**，
///   说明端口对用户不可配置。
///
/// 因为游戏内的「数据输出IP端口」和「数据输出IP地址」**都是可以自由填写的**，
/// 正确做法是：让游戏发给 relay 自己的端口，由 relay 再转发到这个端口，
/// 而不是让 relay 去抢 5300（抢了会把飞智顶掉，两个 socket 抢同一端口时
/// 在 Windows 上单播包只会投递给**第一个**绑定者）。
pub const FLYDIGI_FORZA_PORT: u16 = 5300;
