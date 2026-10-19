//! HUD 的全部可调参数。
//!
//! # 为什么把参数单独建模
//!
//! 目标是「**实时调参仪表**」：所有外观参数都能当场拖动并即刻看到效果，
//! 满意后存成主题文件。这要求参数满足三个条件：
//!
//! 1. **可序列化**：能存成 JSON、能加载回来；
//! 2. **有合理默认值**：任何参数缺失或损坏都能回退，绝不因为配置问题崩溃；
//! 3. **可校验**：外部传入的越界值要被安全地夹紧，而不是画出错乱画面。
//!
//! 这三条都是纯逻辑，因此可以完整单元测试 —— 见本文件底部的测试。
//!
//! # 与参考实现的差异
//!
//! 参考实现的配置是「4 套固定 profile × 9 个部件的像素偏移」
//! （`overlay_layout.json`），可调项少且分成四档，改起来要重启。
//! 这里改为**扁平的参数全集 + 实时生效**，把「档位」变成可由用户
//! 自己保存的多个主题文件。
//!
//! # 法律说明
//!
//! 本模块的设计与数值均为本项目原创。参考实现
//! [O93-3/093_Drift_HUD](https://github.com/O93-3/093_Drift_HUD) 无 LICENSE，
//! 其配置结构与本模块无关，未被参考或复制。

use serde::{Deserialize, Serialize};

/// 参数取值范围与默认值。
///
/// 用一个 trait 把「夹紧」与「默认值」绑在一起，这样新增参数时
/// 只要实现两个方法，校验与回退就自动成立，不会出现"漏夹一个参数"。
pub trait ParamRange {
    /// 允许的最小值。
    const MIN: f32;
    /// 允许的最大值。
    const MAX: f32;
    /// 缺省值。
    const DEFAULT: f32;

    /// 夹紧到合法范围。NaN 回退到 [`ParamRange::DEFAULT`]。
    fn clamp_value(v: f32) -> f32 {
        if v.is_nan() {
            Self::DEFAULT
        } else {
            v.clamp(Self::MIN, Self::MAX)
        }
    }
}

/// 声明一个带范围的 f32 参数类型。
///
/// 生成的类型只用于承载 `MIN` / `MAX` / `DEFAULT` 三个常量，
/// 本身没有实例。
macro_rules! ranged {
    ($name:ident, $min:expr, $max:expr, $def:expr) => {
        #[doc = concat!("范围 [", stringify!($min), ", ", stringify!($max), "]，默认 ", stringify!($def), "。")]
        pub struct $name;
        impl ParamRange for $name {
            const MIN: f32 = $min;
            const MAX: f32 = $max;
            const DEFAULT: f32 = $def;
        }
    };
}

// ---- 主角度表 ----
ranged!(Radius, 40.0, 1200.0, 320.0);
ranged!(ArcWidth, 1.0, 80.0, 18.0);
ranged!(GlowStrength, 0.0, 1.0, 0.35);
ranged!(GlowLayers, 0.0, 8.0, 3.0);

// ---- 透明度 ----
ranged!(Opacity, 0.0, 1.0, 1.0);
// 面板底衬的不透明度。
// **默认值刻意压得很低（0.10）**：HUD 的价值在于「看得见游戏画面」，
// 底衬过重会像贴了一块黑板。文字可读性靠描边解决，而不是靠底色。
ranged!(PanelOpacity, 0.0, 1.0, 0.10);
ranged!(ScanlineAlpha, 0.0, 1.0, 0.06);
ranged!(HalftoneDensity, 0.0, 1.0, 0.0);
// 圆形仪表（G 表）底衬的不透明度。
ranged!(RingBackdropAlpha, 0.0, 1.0, 0.12);
// 弹窗底衬的不透明度。
ranged!(PopupBackdropAlpha, 0.0, 1.0, 0.22);

// ---- 位置与缩放 ----
ranged!(Pos, -4000.0, 4000.0, 0.0);
ranged!(Scale, 0.25, 4.0, 1.0);

// ---- 面板 ----
ranged!(PanelW, 40.0, 1200.0, 260.0);
ranged!(PanelH, 20.0, 900.0, 140.0);
ranged!(BarLength, 20.0, 900.0, 180.0);
ranged!(BarThickness, 1.0, 60.0, 10.0);
ranged!(FontSize, 6.0, 120.0, 16.0);

// ---- G 表 ----
ranged!(GMaxScale, 0.5, 6.0, 2.5);
ranged!(TrailAlpha, 0.0, 1.0, 0.5);

// ---- 事件弹窗 ----
ranged!(PopupSeconds, 0.25, 6.0, 1.15);
ranged!(PopupScale, 0.25, 4.0, 1.0);

// ---- 主角度表细节 ----
ranged!(TickLength, 2.0, 120.0, 14.0);
// 指针长度按半径的比例表示（0.1–1.0）。
ranged!(NeedleLength, 0.1, 1.0, 0.82);

/// 主角度表的参数。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AngleGaugeParams {
    /// 圆心 X（屏幕像素）。
    pub cx: f32,
    /// 圆心 Y（屏幕像素）。
    pub cy: f32,
    /// 半径。
    pub radius: f32,
    /// 弧线宽度。
    pub arc_width: f32,
    /// 刻度数量。
    pub tick_count: u32,
    /// 刻度长度。
    pub tick_length: f32,
    /// 指针长度。
    pub needle_length: f32,
    /// 辉光强度 0–1。
    pub glow_strength: f32,
    /// 辉光层数。
    pub glow_layers: f32,
    /// 整体不透明度。
    pub opacity: f32,
}

impl Default for AngleGaugeParams {
    fn default() -> Self {
        Self {
            cx: 960.0,
            cy: 540.0,
            radius: Radius::DEFAULT,
            arc_width: ArcWidth::DEFAULT,
            tick_count: 11,
            tick_length: 14.0,
            needle_length: 0.82,
            glow_strength: GlowStrength::DEFAULT,
            glow_layers: GlowLayers::DEFAULT,
            opacity: Opacity::DEFAULT,
        }
    }
}

impl AngleGaugeParams {
    /// 把所有字段夹紧到合法范围。
    pub fn sanitize(&mut self) {
        self.cx = Pos::clamp_value(self.cx);
        self.cy = Pos::clamp_value(self.cy);
        self.radius = Radius::clamp_value(self.radius);
        self.arc_width = ArcWidth::clamp_value(self.arc_width);
        self.tick_count = self.tick_count.min(64);
        self.tick_length = TickLength::clamp_value(self.tick_length);
        self.needle_length = NeedleLength::clamp_value(self.needle_length);
        self.glow_strength = GlowStrength::clamp_value(self.glow_strength);
        self.glow_layers = GlowLayers::clamp_value(self.glow_layers);
        self.opacity = Opacity::clamp_value(self.opacity);
    }
}

/// G 表的参数。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GMeterParams {
    /// 圆心 X。
    pub cx: f32,
    /// 圆心 Y。
    pub cy: f32,
    /// 半径。
    pub radius: f32,
    /// 圆环线宽。
    pub ring_width: f32,
    /// 圆边界对应的 G 值。
    pub max_g: f32,
    /// 历史轨迹透明度。
    pub trail_alpha: f32,
    /// 是否显示网格。
    pub show_grid: bool,
    /// 是否显示轨迹。
    pub show_trail: bool,
    /// 钩子圆点半径。
    pub dot_radius: f32,
    /// 圆环底衬的不透明度（0 = 完全透明，只留线条）。
    pub backdrop_alpha: f32,
}

impl Default for GMeterParams {
    fn default() -> Self {
        Self {
            cx: 1560.0,
            cy: 320.0,
            radius: 130.0,
            ring_width: 10.0,
            max_g: GMaxScale::DEFAULT,
            trail_alpha: TrailAlpha::DEFAULT,
            show_grid: true,
            show_trail: true,
            dot_radius: 5.0,
            backdrop_alpha: RingBackdropAlpha::DEFAULT,
        }
    }
}

impl GMeterParams {
    /// 夹紧。
    pub fn sanitize(&mut self) {
        self.cx = Pos::clamp_value(self.cx);
        self.cy = Pos::clamp_value(self.cy);
        self.radius = Radius::clamp_value(self.radius);
        self.ring_width = BarThickness::clamp_value(self.ring_width);
        self.max_g = GMaxScale::clamp_value(self.max_g);
        self.trail_alpha = TrailAlpha::clamp_value(self.trail_alpha);
        self.dot_radius = BarThickness::clamp_value(self.dot_radius);
        self.backdrop_alpha = RingBackdropAlpha::clamp_value(self.backdrop_alpha);
    }
}

/// 条形面板的参数（用于输入、滑移、FLOW/RISK 等）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BarPanelParams {
    /// 面板左上角 X。
    pub x: f32,
    /// 面板左上角 Y。
    pub y: f32,
    /// 面板宽度。
    pub w: f32,
    /// 面板高度。
    pub h: f32,
    /// 条形长度。
    pub bar_length: f32,
    /// 条形厚度。
    pub bar_thickness: f32,
    /// 标题字号。
    pub title_size: f32,
    /// 数值字号。
    pub value_size: f32,
    /// 面板底色不透明度。
    pub panel_opacity: f32,
    /// 扫描线强度。
    pub scanline_alpha: f32,
    /// 是否显示面板边框与角括号。
    pub show_frame: bool,
    /// 是否显示数值文本。
    pub show_value: bool,
}

impl Default for BarPanelParams {
    fn default() -> Self {
        Self {
            x: 40.0,
            y: 40.0,
            w: PanelW::DEFAULT,
            h: PanelH::DEFAULT,
            bar_length: BarLength::DEFAULT,
            bar_thickness: BarThickness::DEFAULT,
            title_size: FontSize::DEFAULT,
            value_size: FontSize::DEFAULT,
            panel_opacity: PanelOpacity::DEFAULT,
            scanline_alpha: ScanlineAlpha::DEFAULT,
            show_frame: true,
            show_value: true,
        }
    }
}

impl BarPanelParams {
    /// 夹紧。
    pub fn sanitize(&mut self) {
        self.x = Pos::clamp_value(self.x);
        self.y = Pos::clamp_value(self.y);
        self.w = PanelW::clamp_value(self.w);
        self.h = PanelH::clamp_value(self.h);
        self.bar_length = BarLength::clamp_value(self.bar_length);
        self.bar_thickness = BarThickness::clamp_value(self.bar_thickness);
        self.title_size = FontSize::clamp_value(self.title_size);
        self.value_size = FontSize::clamp_value(self.value_size);
        self.panel_opacity = PanelOpacity::clamp_value(self.panel_opacity);
        self.scanline_alpha = ScanlineAlpha::clamp_value(self.scanline_alpha);
    }
}

/// 事件弹窗的参数。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PopupParams {
    /// 弹窗中心 X。
    pub cx: f32,
    /// 弹窗中心 Y。
    pub cy: f32,
    /// 显示时长倍率。
    pub seconds_scale: f32,
    /// 整体缩放倍率。
    pub scale: f32,
    /// hero 级弹窗的额外放大倍率。
    pub hero_scale: f32,
    /// 描边层数（文字外发光）。
    pub outline_layers: u32,
    /// 是否显示长按进度条。
    pub show_hold_meter: bool,
    /// 弹窗底衬的不透明度（0 = 完全不画底衬）。
    pub backdrop_alpha: f32,
}

impl Default for PopupParams {
    fn default() -> Self {
        Self {
            // 放在仪表弧线之上、屏幕上方空白区：既不挡中心读数，也不出画面
            cx: 960.0,
            cy: 300.0,
            seconds_scale: 1.0,
            scale: PopupScale::DEFAULT,
            hero_scale: 1.35,
            outline_layers: 3,
            show_hold_meter: true,
            backdrop_alpha: PopupBackdropAlpha::DEFAULT,
        }
    }
}

impl PopupParams {
    /// 夹紧。
    pub fn sanitize(&mut self) {
        self.cx = Pos::clamp_value(self.cx);
        self.cy = Pos::clamp_value(self.cy);
        self.seconds_scale = PopupSeconds::clamp_value(self.seconds_scale);
        self.scale = PopupScale::clamp_value(self.scale);
        self.hero_scale = PopupScale::clamp_value(self.hero_scale);
        self.outline_layers = self.outline_layers.min(8);
        self.backdrop_alpha = PopupBackdropAlpha::clamp_value(self.backdrop_alpha);
    }
}

/// 渲染性能相关参数。
///
/// 这一组是本项目相对参考实现的核心差异所在。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PerfParams {
    /// 是否启用脏区重绘。
    ///
    /// `true`（默认）：只有数值变化的组件才重绘，静止时**整帧跳过**。
    /// `false`：每帧无条件全屏重绘 —— 即参考实现的行为，仅用于对比测试。
    pub dirty_region_redraw: bool,
    /// 静止时是否降低刷新率。
    pub idle_throttle: bool,
    /// 静止时的刷新率上限（Hz）。仅在 `idle_throttle` 为真时生效。
    pub idle_fps: f32,
    /// 活动时的刷新率上限（Hz）。
    pub active_fps: f32,
    /// 弧线的分段数（越大越圆滑，越费 CPU）。
    pub arc_segments: u32,
}

impl Default for PerfParams {
    fn default() -> Self {
        Self {
            dirty_region_redraw: true,
            idle_throttle: true,
            idle_fps: 4.0,
            active_fps: 60.0,
            arc_segments: 64,
        }
    }
}

impl PerfParams {
    /// 夹紧。
    pub fn sanitize(&mut self) {
        self.idle_fps = self.idle_fps.clamp(0.5, 240.0);
        self.active_fps = self.active_fps.clamp(1.0, 240.0);
        self.arc_segments = self.arc_segments.clamp(6, 512);
        if self.idle_fps > self.active_fps {
            self.idle_fps = self.active_fps;
        }
    }
}

/// 界面语言。
///
/// HUD 上的标签（油门/刹车/流畅度/风险…）与事件弹窗文案都会随它切换。
/// 默认中文 —— 目标用户是中文玩家，英文名词堆在仪表上会增加认知负担。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// 简体中文（默认）。
    #[default]
    Zh,
    /// 英文。
    En,
}

impl Language {
    /// 从字符串解析；无法识别时回退到默认（中文），不报错。
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "en" | "eng" | "english" => Language::En,
            _ => Language::Zh,
        }
    }

    /// 反解成配置里的字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Language::Zh => "zh",
            Language::En => "en",
        }
    }

    /// 供界面显示的名称。
    pub fn display_name(self) -> &'static str {
        match self {
            Language::Zh => "中文",
            Language::En => "English",
        }
    }
}

/// 遥测接收端点（HUD 监听什么地址、什么端口）。
///
/// 之所以放进配置而不是只留命令行参数：用户在 GUI 里改完就该长期生效，
/// 不该每次启动都敲一遍参数。命令行仍可覆盖（优先级更高）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkConfig {
    /// 监听地址。默认 `127.0.0.1`。
    ///
    /// 一般只填本地回环；填 `0.0.0.0` 可接收局域网其他机器的遥测，
    /// 但这会把端口暴露到网络，需自行评估。
    pub listen_ip: String,
    /// 监听端口。默认 5302（relay 的第二个扇出目标）。
    pub listen_port: u16,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            listen_ip: "127.0.0.1".to_string(),
            listen_port: 5302,
        }
    }
}

impl NetworkConfig {
    /// 校验并规范化。
    ///
    /// * 地址为空 → 回退 `127.0.0.1`；
    /// * 端口为 0 → 回退默认端口；
    /// * 地址过长 → 截断（防止畸形配置撑爆内存）。
    pub fn sanitize(&mut self) {
        let ip = self.listen_ip.trim();
        if ip.is_empty() || ip.len() > 64 {
            self.listen_ip = "127.0.0.1".to_string();
        } else {
            self.listen_ip = ip.to_string();
        }
        if self.listen_port == 0 {
            self.listen_port = 5302;
        }
    }

    /// 组合成 `ip:port` 字符串。
    pub fn endpoint(&self) -> String {
        format!("{}:{}", self.listen_ip, self.listen_port)
    }
}

/// HUD 的完整参数集合。
///
/// 含 `String`（主题名、监听地址），因此**不是** `Copy`；但它是 `Clone`，
/// 渲染层可以克隆一份快照后自行取用，无需加锁。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 参数格式版本，用于将来做迁移。
    pub version: u32,
    /// 主题名称（保存时用）。
    pub theme_name: String,
    /// 界面语言（中文 / 英文）。
    pub language: Language,
    /// 遥测接收端点。
    pub network: NetworkConfig,
    /// 全局缩放。
    pub global_scale: f32,
    /// 全局水平偏移。
    pub offset_x: f32,
    /// 全局垂直偏移。
    pub offset_y: f32,
    /// 主角度表。
    pub angle: AngleGaugeParams,
    /// G 表。
    pub g_meter: GMeterParams,
    /// 输入条面板（油门/刹车/离合/手刹/方向）。
    pub input_panel: BarPanelParams,
    /// 后轴滑移面板。
    pub slip_panel: BarPanelParams,
    /// FLOW / RISK 面板。
    pub flow_panel: BarPanelParams,
    /// 事件弹窗。
    pub popup: PopupParams,
    /// 性能相关。
    pub perf: PerfParams,
}

/// 当前参数格式版本。
pub const SETTINGS_VERSION: u32 = 1;

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            theme_name: "default".to_string(),
            language: Language::default(),
            network: NetworkConfig::default(),
            global_scale: 1.0,
            offset_x: 0.0,
            offset_y: 0.0,
            angle: AngleGaugeParams::default(),
            g_meter: GMeterParams::default(),
            input_panel: BarPanelParams {
                x: 40.0,
                y: 40.0,
                ..Default::default()
            },
            slip_panel: BarPanelParams {
                x: 40.0,
                y: 250.0,
                ..Default::default()
            },
            flow_panel: BarPanelParams {
                x: 40.0,
                y: 470.0,
                ..Default::default()
            },
            popup: PopupParams::default(),
            perf: PerfParams::default(),
        }
    }
}

impl Settings {
    /// 把所有字段夹紧到合法范围，并修正参数间的约束。
    ///
    /// **任何外部来源的参数（文件、命令行、UI 拖动）都必须先过这一步。**
    /// 这样渲染层就可以假定参数是合法的，不必到处写防御代码。
    ///
    /// 返回 `true` 表示发生了修正（调用方可用于提示用户或标记需要保存）。
    pub fn sanitize(&mut self) -> bool {
        // 先记录「受影响字段」的原始值用于变更检测。
        // 不克隆整个 Settings —— 里面有多处 String，克隆是浪费；而且逐个比较
        // 字段能精确表达「夹紧只影响这些字段」。
        // 注意：Rust 只为标准库元组实现到 12 个元素的 PartialEq，
        // 所以这里嵌成三层，而不是写成一个 14 元组。
        let before = (
            (self.global_scale, self.offset_x, self.offset_y),
            (
                self.angle,
                self.g_meter,
                self.input_panel,
                self.slip_panel,
                self.flow_panel,
                self.popup,
                self.perf,
            ),
            (
                self.version,
                self.theme_name.clone(),
                self.language,
                self.network.clone(),
            ),
        );

        self.global_scale = Scale::clamp_value(self.global_scale);
        self.offset_x = Pos::clamp_value(self.offset_x);
        self.offset_y = Pos::clamp_value(self.offset_y);
        self.angle.sanitize();
        self.g_meter.sanitize();
        self.input_panel.sanitize();
        self.slip_panel.sanitize();
        self.flow_panel.sanitize();
        self.popup.sanitize();
        self.perf.sanitize();
        self.network.sanitize();

        if self.version == 0 {
            self.version = SETTINGS_VERSION;
        }
        if self.theme_name.chars().count() > 64 {
            self.theme_name = self.theme_name.chars().take(64).collect();
        }

        let after = (
            (self.global_scale, self.offset_x, self.offset_y),
            (
                self.angle,
                self.g_meter,
                self.input_panel,
                self.slip_panel,
                self.flow_panel,
                self.popup,
                self.perf,
            ),
            (
                self.version,
                self.theme_name.clone(),
                self.language,
                self.network.clone(),
            ),
        );

        before != after
    }

    /// 从 JSON 字符串加载，失败时回退到默认值。
    ///
    /// **绝不因为配置损坏而让程序起不来** —— 这是 HUD 类工具的基本要求：
    /// 用户宁可看到默认外观，也不愿意游戏里突然没有 HUD 还找不到原因。
    pub fn from_json(s: &str) -> (Self, Option<String>) {
        match serde_json::from_str::<Settings>(s) {
            Ok(mut v) => {
                let fixed = v.sanitize();
                let note = if fixed {
                    Some("部分参数超出合法范围，已自动修正".to_string())
                } else {
                    None
                };
                (v, note)
            }
            Err(e) => (
                Settings::default(),
                Some(format!("配置解析失败，已回退到默认值: {e}")),
            ),
        }
    }

    /// 序列化成带缩进的 JSON。
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// 当前生效的刷新率上限（根据是否有活动决定）。
    ///
    /// `dirty` 表示本帧是否有组件发生变化。
    pub fn target_fps(&self, dirty: bool) -> f32 {
        if dirty || !self.perf.idle_throttle {
            self.perf.active_fps
        } else {
            self.perf.idle_fps
        }
    }

    /// 两帧之间的目标间隔（毫秒）。
    pub fn frame_interval_ms(&self, dirty: bool) -> f32 {
        let fps = self.target_fps(dirty);
        if fps <= 0.0 {
            1000.0
        } else {
            1000.0 / fps
        }
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    #[test]
    fn 默认参数本身必须合法() {
        // 这条很重要：默认值如果自己就越界，sanitize 会每次都报告"已修正"，
        // 用户一启动就看到莫名其妙的提示。
        let mut s = Settings::default();
        assert!(!s.sanitize(), "默认参数不应需要修正");
    }

    #[test]
    fn 所有默认值都落在各自范围内() {
        assert!((Radius::MIN..=Radius::MAX).contains(&Radius::DEFAULT));
        assert!((ArcWidth::MIN..=ArcWidth::MAX).contains(&ArcWidth::DEFAULT));
        assert!((GMaxScale::MIN..=GMaxScale::MAX).contains(&GMaxScale::DEFAULT));
        assert!((PerfParams::default().active_fps).is_finite());
        assert!(PerfParams::default().arc_segments >= 6);
    }

    #[test]
    fn 越界参数被夹紧() {
        let mut s = Settings::default();
        s.angle.radius = 99999.0;
        s.angle.arc_width = -50.0;
        s.angle.opacity = 5.0;
        s.global_scale = 0.001;
        s.perf.arc_segments = 99999;
        s.perf.active_fps = 1000.0;

        assert!(s.sanitize(), "应报告发生了修正");

        assert_eq!(s.angle.radius, Radius::MAX);
        assert_eq!(s.angle.arc_width, ArcWidth::MIN);
        assert_eq!(s.angle.opacity, 1.0);
        assert_eq!(s.global_scale, Scale::MIN);
        assert_eq!(s.perf.arc_segments, 512);
        assert_eq!(s.perf.active_fps, 240.0);
    }

    #[test]
    fn NaN参数回退到默认值而不是产生非法值() {
        let mut s = Settings::default();
        s.angle.radius = f32::NAN;
        s.g_meter.max_g = f32::NAN;
        s.global_scale = f32::NAN;
        s.popup.scale = f32::NAN;

        s.sanitize();

        assert_eq!(s.angle.radius, Radius::DEFAULT);
        assert_eq!(s.g_meter.max_g, GMaxScale::DEFAULT);
        assert_eq!(s.global_scale, Scale::DEFAULT);
        assert_eq!(s.popup.scale, PopupScale::DEFAULT);
        assert!(s.angle.radius.is_finite());
    }

    #[test]
    fn 无穷大也会被夹紧() {
        let mut s = Settings::default();
        s.angle.radius = f32::INFINITY;
        s.angle.cy = f32::NEG_INFINITY;
        s.sanitize();
        assert_eq!(s.angle.radius, Radius::MAX);
        assert_eq!(s.angle.cy, Pos::MIN);
        assert!(s.angle.cy.is_finite());
    }

    #[test]
    fn 静止时刷新率被降低() {
        let s = Settings::default();
        assert_eq!(s.target_fps(true), 60.0, "有变化时应用活动刷新率");
        assert_eq!(s.target_fps(false), 4.0, "静止时应用低刷新率");
        assert!(s.frame_interval_ms(false) > s.frame_interval_ms(true));
    }

    #[test]
    fn 关闭静止节流后始终用活动刷新率() {
        let mut s = Settings::default();
        s.perf.idle_throttle = false;
        assert_eq!(s.target_fps(false), 60.0);
    }

    #[test]
    fn idle_fps不会超过active_fps() {
        let mut s = Settings::default();
        s.perf.idle_fps = 200.0;
        s.perf.active_fps = 30.0;
        s.sanitize();
        assert!(
            s.perf.idle_fps <= s.perf.active_fps,
            "静止刷新率不应高于活动刷新率"
        );
    }

    #[test]
    fn json往返无损() {
        let mut s = Settings {
            theme_name: "我的主题".to_string(),
            ..Default::default()
        };
        s.angle.radius = 420.5;
        s.g_meter.show_trail = false;
        s.perf.dirty_region_redraw = false;

        let json = s.to_json();
        let (back, note) = Settings::from_json(&json);
        assert!(note.is_none(), "合法 JSON 不应产生提示: {note:?}");
        assert_eq!(back, s, "JSON 往返必须无损");
    }

    #[test]
    fn 损坏的json回退到默认值而不panic() {
        for bad in [
            "",
            "{",
            "not json at all",
            "{\"angle\": 12345}",
            "{\"angle\":{\"radius\":\"文本\"}}",
            "{\"angle\":{\"radius\":true}}",
            "12345",
        ] {
            let (s, note) = Settings::from_json(bad);
            assert!(note.is_some(), "损坏的输入 `{bad}` 应产生回退提示");
            assert_eq!(s, Settings::default(), "应回退到默认值");
        }
    }

    #[test]
    fn 空数组与null都能被容忍并取默认值() {
        // 这两个不是"损坏输入"，而是 serde 会接受的写法：
        // * `[]`   —— Vec 反序列化时空序列会填成默认元素，于是全部取默认值；
        // * `null` —— 被当作"整个结构缺失"。
        // 关键是**不能 panic，也不能产生非法参数**。
        for tolerant in ["[]", "null", "{}"] {
            let (s, _note) = Settings::from_json(tolerant);
            let mut expect = Settings::default();
            expect.sanitize();
            assert_eq!(s, expect, "`{tolerant}` 应得到合法默认参数");
        }
    }

    #[test]
    fn 缺失字段用默认值补齐() {
        // 只给一个字段，其余应全部取默认
        let (s, _) = Settings::from_json("{\"angle\":{\"radius\":500.0}}");
        assert_eq!(s.angle.radius, 500.0);
        assert_eq!(s.angle.arc_width, ArcWidth::DEFAULT, "缺失字段应取默认值");
        assert_eq!(s.g_meter.radius, GMeterParams::default().radius);
    }

    #[test]
    fn 未知字段被忽略而不是报错() {
        let (s, note) = Settings::from_json("{\"未来字段\": 123, \"angle\":{\"radius\":300.0}}");
        assert_eq!(s.angle.radius, 300.0);
        // serde 默认允许未知字段，因此不应有错误提示
        assert!(note.is_none());
    }

    #[test]
    fn 解析越界的json会被夹紧并给出提示() {
        let (s, note) = Settings::from_json("{\"angle\":{\"radius\":999999.0}}");
        assert_eq!(s.angle.radius, Radius::MAX);
        assert!(note.is_some(), "发生修正时应给出提示");
    }

    #[test]
    fn 过长的主题名被截断() {
        let mut s = Settings {
            theme_name: "x".repeat(500),
            ..Default::default()
        };
        s.sanitize();
        assert_eq!(s.theme_name.chars().count(), 64);
    }

    #[test]
    fn 版本为0时被补成当前版本() {
        let (s, _) = Settings::from_json("{\"version\":0}");
        assert_eq!(s.version, SETTINGS_VERSION);
    }

    #[test]
    fn 三次sanitize结果稳定() {
        // 夹紧必须是幂等的：否则每次加载都报告"已修正"，
        // 或者出现"夹紧后仍然越界"的诡异状态。
        let mut s = Settings::default();
        s.angle.radius = -999.0;
        s.perf.arc_segments = 1;
        s.sanitize();
        let once = s.clone();
        assert!(!s.sanitize(), "第二次不应再报告修正");
        assert_eq!(s, once);
    }

    #[test]
    fn 刻度数量有上限避免画出天量刻度() {
        let mut s = Settings::default();
        s.angle.tick_count = u32::MAX;
        s.sanitize();
        assert!(s.angle.tick_count <= 64);
    }

    #[test]
    fn 弹窗描边层数有上限() {
        let mut s = Settings::default();
        s.popup.outline_layers = u32::MAX;
        s.sanitize();
        assert!(s.popup.outline_layers <= 8);
    }

    #[test]
    fn 默认开启脏区重绘() {
        // 这是本项目的核心资源优化，默认必须是开的。
        let s = Settings::default();
        assert!(s.perf.dirty_region_redraw, "默认应启用脏区重绘");
        assert!(s.perf.idle_throttle, "默认应启用静止节流");
    }
}
