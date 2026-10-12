//! HUD 的视觉主题与几何计算。
//!
//! # 为什么把「几何」单独拆出来
//!
//! 渲染层最难验证的部分是「画出来的东西对不对」，但其中**有一半是可测的纯数学**：
//! 角度→弧线坐标、G 值→圆内坐标、滑移→条长、字号→布局。这些只要给定输入就
//! 必然有确定输出，因此可以完全单元测试 —— 不需要 GPU、不需要窗口、
//! 不需要连着游戏。
//!
//! 不可测的那一半（描边、辉光、网点、扫描线等质感叠加）留在渲染后端里，
//! 那部分只能靠肉眼核对，见 `docs/架构决策.md` 的 ADR-011。
//!
//! # 参数来源与法律边界
//!
//! 本模块的颜色、几何比例、阈值等**功能性参数**参考自
//! [O93-3/093_Drift_HUD](https://github.com/O93-3/093_Drift_HUD) 的公开视觉表现
//! （其 `STREET_*` / `ANGLE_*` 颜色常量与仪表布局）。该仓库**没有 LICENSE**
//! （GitHub 标记 `NOASSERTION`），因此本项目：
//!
//! * ✅ 只提取**颜色数值、几何比例、阈值**这类功能性/事实性参数；
//! * ❌ **没有**复制或翻译其任何绘制代码、路径构造、坐标常量表；
//! * ❌ **没有**使用其 `cars.json` 等数据文件。
//!
//! 若需要像素级还原其全部质感，正确路径是联系原作者取得授权。

// ============================================================================
// 颜色
// ============================================================================

/// 32 位 BGRA 颜色（与 Windows `UpdateLayeredWindow` 的位图格式一致）。
///
/// 用 BGRA 而不是 RGBA：Windows 的分层窗口位图按 BGRA 排列，
/// 直接在定义处就用目标格式可以省掉每次绘制的通道交换。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct Bgra {
    /// 蓝通道。
    pub b: u8,
    /// 绿通道。
    pub g: u8,
    /// 红通道。
    pub r: u8,
    /// Alpha 通道（0 = 全透明，255 = 全不透明）。
    pub a: u8,
}

impl Bgra {
    /// 由 RGB 与 alpha 构造。
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { b, g, r, a }
    }

    /// 由不透明 RGB 构造。
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::new(r, g, b, 255)
    }

    /// 完全透明的颜色（用作分层窗口的擦除底色）。
    pub const TRANSPARENT: Self = Self::new(0, 0, 0, 0);

    /// 返回一个改了 alpha 的副本。
    pub const fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }

    /// 按 `t`（0–255）在本色与 `other` 之间线性插值。
    ///
    /// 用于绘制渐变（例如 G 表圆环的静态圈→动态圈过渡）。
    pub fn lerp(self, other: Self, t: u8) -> Self {
        let f = |a: u8, b: u8| -> u8 {
            let a = a as u32;
            let b = b as u32;
            let t = t as u32;
            ((a * (255 - t) + b * t) / 255) as u8
        };
        Self {
            b: f(self.b, other.b),
            g: f(self.g, other.g),
            r: f(self.r, other.r),
            a: f(self.a, other.a),
        }
    }

    /// 打包成 `0xAARRGGBB` 形式（GDI 的 `COLORREF` 是 `0x00BBGGRR`，
    /// 但文本/画笔常用 ARGB，故单独提供）。
    pub const fn to_argb(self) -> u32 {
        ((self.a as u32) << 24) | ((self.r as u32) << 16) | ((self.g as u32) << 8) | (self.b as u32)
    }

    /// 打包成 GDI 的 `COLORREF`（`0x00BBGGRR`）。
    pub const fn to_colorref(self) -> u32 {
        ((self.b as u32) << 16) | ((self.g as u32) << 8) | (self.r as u32)
    }
}

/// HUD 配色主题。
///
/// 命名沿用参考实现的语义（为什么叫 `street_*` 只是为了对照方便，
/// 数值本身是功能性参数）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// 主弧线：左侧（负角度）颜色。
    pub angle_left_ice: Bgra,
    /// 主弧线：右侧（正角度）颜色。
    pub angle_right_rose: Bgra,
    /// 接近极限的琥珀色。
    pub angle_limit_amber: Bgra,
    /// 危险橙红。
    pub angle_danger_orange: Bgra,
    /// 冷白强调色。
    pub accent_white: Bgra,
    /// 提亮的近白色。
    pub text_white: Bgra,
    /// 直播用橙色。
    pub accent_amber: Bgra,
    /// 直播用薄荷绿。
    pub accent_mint: Bgra,
    /// 炭黑（面板底色）。
    pub charcoal: Bgra,
    /// 次要文字。
    pub text_dim: Bgra,
    /// 安全/良好状态色（薄荷绿偏冷）。
    pub state_good: Bgra,
    /// 警告状态色（琥珀）。
    pub state_warn: Bgra,
    /// 危险状态色（橙红）。
    pub state_danger: Bgra,
}

impl Default for Theme {
    fn default() -> Self {
        Self::street()
    }
}

impl Theme {
    /// 默认主题的原始颜色值。
    ///
    /// 这些是**功能性命中色**：左侧冷蓝、右侧玫红用于区分漂移方向，
    /// 琥珀/橙红用于风险分级。数值参考自参考实现的公开视觉表现。
    pub const fn street() -> Self {
        Self {
            // #55DFFF — 左侧冷冰蓝
            angle_left_ice: Bgra::rgb(0x55, 0xDF, 0xFF),
            // #FF3F9B — 右侧玫红
            angle_right_rose: Bgra::rgb(0xFF, 0x3F, 0x9B),
            // #FFB84A — 极限琥珀
            angle_limit_amber: Bgra::rgb(0xFF, 0xB8, 0x4A),
            // #FF4A3D — 危险橙红
            angle_danger_orange: Bgra::rgb(0xFF, 0x4A, 0x3D),
            // #EEF6FF — 冷白强调
            accent_white: Bgra::rgb(0xEE, 0xF6, 0xFF),
            // #F8FCFF — 提亮近白
            text_white: Bgra::rgb(0xF8, 0xFC, 0xFF),
            // #FFB836 — 直播橙
            accent_amber: Bgra::rgb(0xFF, 0xB8, 0x36),
            // #52FFEB — 直播薄荷
            accent_mint: Bgra::rgb(0x52, 0xFF, 0xEB),
            // #05080D — 炭黑
            charcoal: Bgra::rgb(0x05, 0x08, 0x0D),
            // #8A93A0 — 次要文字（灰蓝）
            text_dim: Bgra::rgb(0x8A, 0x93, 0xA0),
            state_good: Bgra::rgb(0x52, 0xFF, 0xEB),
            state_warn: Bgra::rgb(0xFF, 0xB8, 0x4A),
            state_danger: Bgra::rgb(0xFF, 0x4A, 0x3D),
        }
    }

    /// 风格标签对应的强调色。
    pub fn style_color(&self, style: fh6_core::drift::Style) -> Bgra {
        use fh6_core::drift::Style;
        match style {
            Style::Danger => self.state_danger,
            Style::Edge => self.angle_limit_amber,
            Style::Wild => self.angle_right_rose,
            Style::Locked => self.accent_mint,
            Style::Deep => self.angle_left_ice,
            Style::Smooth => self.accent_mint,
            Style::Save => self.accent_amber,
            Style::Kick | Style::Brake | Style::Entry => self.accent_amber,
            Style::Clean | Style::Grip => self.text_dim,
        }
    }

    /// 风险标签对应的颜色。
    pub fn risk_color(&self, label: fh6_core::drift::RiskLabel) -> Bgra {
        use fh6_core::drift::RiskLabel;
        match label {
            RiskLabel::Safe => self.state_good,
            RiskLabel::Edge => self.accent_mint,
            RiskLabel::Risk => self.angle_limit_amber,
            RiskLabel::Max => self.state_danger,
        }
    }

    /// FLOW 质量标签对应的颜色。
    pub fn flow_color(&self, q: fh6_core::drift::FlowQuality) -> Bgra {
        use fh6_core::drift::FlowQuality;
        match q {
            FlowQuality::Build => self.text_dim,
            FlowQuality::Flow => self.angle_left_ice,
            FlowQuality::Chase => self.accent_amber,
            FlowQuality::Smooth => self.accent_mint,
            FlowQuality::Locked => self.text_white,
        }
    }
}

// ============================================================================
// 环形/弧形几何
// ============================================================================

/// 角度制常量：一整圈。
pub const DEG_FULL: f32 = 360.0;

/// 把一个「仪表角度」转成屏幕弧度。
///
/// # 约定
///
/// * 输入 `deg`：**仪表角度**，0 = 正上方（12 点方向），正数顺时针。
///   这与「漂移角」的正负含义一致（正 = 右打方向）。
/// * 输出：标准数学弧度，供三角函数直接使用。
///
/// 推导：屏幕坐标 Y 轴向下，而数学上 0 弧度指向 +X。
/// 「正上方」对应数学角 `-90°`（即 `-π/2`）。顺时针为正即角度增加，
/// 因此 `数学角 = deg + (-90°)`。
///
/// 例：`deg = 0` → `-π/2`（上方 ✔）；
///     `deg = 90` → `0`（右侧 ✔，因为 Y 向下时 0 弧度指向 +X = 右）。
pub fn gauge_deg_to_rad(deg: f32) -> f32 {
    (deg - 90.0).to_radians()
}

/// 仪表角度 → 屏幕坐标点。
///
/// `cx` / `cy` 是圆心，`radius` 是半径。
pub fn gauge_point(cx: f32, cy: f32, radius: f32, deg: f32) -> (f32, f32) {
    let rad = gauge_deg_to_rad(deg);
    (cx + radius * rad.cos(), cy + radius * rad.sin())
}

/// 由「漂移角」得到主仪表的指针角度。
///
/// 漂移角范围 ±90°，映射到仪表 ±`SWEEP_HALF_DEG`。
///
/// # 为什么要夹紧
///
/// 输入若超出 ±90（理论上不该发生，因为派生层已钳制），这里再夹一次，
/// 避免指针跑到表盘外面。
pub const SWEEP_HALF_DEG: f32 = 90.0;

/// 漂移角 → 仪表角度。
pub fn drift_angle_to_gauge(angle_deg: f32) -> f32 {
    let a = if angle_deg.is_nan() {
        0.0
    } else {
        angle_deg.clamp(-SWEEP_HALF_DEG, SWEEP_HALF_DEG)
    };
    // 仪表总跨度 = 2 * SWEEP_HALF_DEG = 180°，即半圆
    a
}

/// 一个圆环扇段（用于画弧）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArcSpec {
    /// 起始仪表角度（度）。
    pub start_deg: f32,
    /// 扫过角度（度），可为负。
    pub sweep_deg: f32,
    /// 半径。
    pub radius: f32,
    /// 线宽。
    pub width: f32,
}

impl ArcSpec {
    /// 由「值 0–1」构造一个从 `from_deg` 扫向 `to_deg` 的弧。
    ///
    /// `t` 会被夹到 0–1，因此超范围输入不会画出越界的弧。
    pub fn from_fraction(from_deg: f32, to_deg: f32, t: f32, radius: f32, width: f32) -> Self {
        let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
        Self {
            start_deg: from_deg,
            sweep_deg: (to_deg - from_deg) * t,
            radius,
            width,
        }
    }

    /// 结束角度。
    pub fn end_deg(&self) -> f32 {
        self.start_deg + self.sweep_deg
    }

    /// 按 `segments` 段离散化成折线点列，写入 `out`。
    ///
    /// 不用贝塞尔曲线而用折线：屏幕上的弧半径不大（通常 < 300 px），
    /// 分段数取到 64 以上时肉眼已看不出多边形。
    /// 返回写入的点数。
    pub fn polyline(&self, cx: f32, cy: f32, segments: usize, out: &mut [(f32, f32)]) -> usize {
        let segments = segments.max(1);
        let n = (segments + 1).min(out.len());
        for (i, slot) in out.iter_mut().enumerate().take(n) {
            let t = i as f32 / segments as f32;
            let deg = self.start_deg + self.sweep_deg * t;
            *slot = gauge_point(cx, cy, self.radius, deg);
        }
        n
    }

    /// 弧线上的一个点（`t` 为 0–1）。
    pub fn point_at(&self, cx: f32, cy: f32, t: f32) -> (f32, f32) {
        let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
        gauge_point(cx, cy, self.radius, self.start_deg + self.sweep_deg * t)
    }

    /// 弧线在 `t` 处的**切向单位矢量**与**法向单位矢量**。
    ///
    /// 用于把指针/刻度按切向摆放（参考实现的仪表指针就是切向定位的）。
    /// 返回 `(tangent, normal)`。
    pub fn tangent_normal(&self, t: f32) -> ((f32, f32), (f32, f32)) {
        let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
        let deg = self.start_deg + self.sweep_deg * t;
        let rad = gauge_deg_to_rad(deg);
        // 位置对角度求导： d/dθ (cos, sin) = (-sin, cos)，再乘 sweep 的符号
        let sign = if self.sweep_deg >= 0.0 { 1.0 } else { -1.0 };
        let tx = -rad.sin() * sign;
        let ty = rad.cos() * sign;
        // 法向 = 切向旋转 90°
        let nx = -ty;
        let ny = tx;
        ((tx, ty), (nx, ny))
    }
}

/// 分段刻度：把一段弧等分成 `count` 份，返回每个刻度的仪表角度。
///
/// 写入 `out`，返回写入个数。
pub fn tick_angles(from_deg: f32, to_deg: f32, count: usize, out: &mut [f32]) -> usize {
    let n = count.min(out.len());
    if n == 0 {
        return 0;
    }
    if n == 1 {
        out[0] = from_deg;
        return 1;
    }
    for (i, slot) in out.iter_mut().enumerate().take(n) {
        let t = i as f32 / (n - 1) as f32;
        *slot = from_deg + (to_deg - from_deg) * t;
    }
    n
}

// ============================================================================
// 圆内的 G 表几何
// ============================================================================

/// G 表圆环的内外半径比例（相对半径）。
pub const G_RING_INNER_RATIO: f32 = 0.62;
/// G 表圆环的外半径比例。
pub const G_RING_OUTER_RATIO: f32 = 0.92;
/// G 表历史轨迹保留的点数（与参考实现一致：30 帧）。
pub const G_TRAIL_LEN: usize = 30;

/// 把 G 值映射成圆内坐标。
///
/// * `g_lat` — 横向 G，**正 = 右转**（见 [`crate::derived::g_forces`] 的符号约定）。
/// * `g_long` — 纵向 G，正 = 加速。
/// * `max_g` — 圆边界对应的 G 值（通常 2.5）。
///
/// 屏幕约定：横向 G 映射到 X 轴（右为正），纵向 G 映射到 Y 轴
/// **向上为正**（因此要取负，因为屏幕 Y 向下）。
pub fn g_point(cx: f32, cy: f32, radius: f32, g_lat: f32, g_long: f32, max_g: f32) -> (f32, f32) {
    let max_g = if max_g.abs() < 1e-6 { 1.0 } else { max_g.abs() };
    let lat = if g_lat.is_nan() { 0.0 } else { g_lat };
    let lon = if g_long.is_nan() { 0.0 } else { g_long };

    // 先按比例算单位圆内坐标，再夹到单位圆内（避免画出圆外）
    let mut x = lat / max_g;
    let mut y = -lon / max_g; // 屏幕 Y 向下，纵向加速向上
    let len = (x * x + y * y).sqrt();
    if len > 1.0 {
        x /= len;
        y /= len;
    }

    (cx + x * radius, cy + y * radius)
}

/// G 表圆环半径（由圆心半径算出内外半径）。
pub fn g_ring_radii(radius: f32) -> (f32, f32) {
    (radius * G_RING_INNER_RATIO, radius * G_RING_OUTER_RATIO)
}

/// 固定容量、零分配的二维点环缓冲。
///
/// 用于 G 值历史轨迹：每帧压入一个点，满了就覆盖最旧的。
/// 参考实现用 `Vec` + `[-30:]` 切片（每次重新分配），这里改为定长数组，
/// 热路径完全不分配。
#[derive(Debug, Clone)]
pub struct TrailBuf<const N: usize> {
    points: [(f32, f32); N],
    len: usize,
    next: usize,
}

impl<const N: usize> Default for TrailBuf<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> TrailBuf<N> {
    /// 新建空缓冲。
    pub const fn new() -> Self {
        Self {
            points: [(0.0, 0.0); N],
            len: 0,
            next: 0,
        }
    }

    /// 当前点数。
    pub fn len(&self) -> usize {
        self.len
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 容量。
    pub const fn capacity(&self) -> usize {
        N
    }

    /// 压入一个点，满了则覆盖最旧的。
    pub fn push(&mut self, p: (f32, f32)) {
        if N == 0 {
            return;
        }
        self.points[self.next] = p;
        self.next = (self.next + 1) % N;
        if self.len < N {
            self.len += 1;
        }
    }

    /// 清空。
    pub fn clear(&mut self) {
        self.len = 0;
        self.next = 0;
    }

    /// 按「最旧 → 最新」的顺序取第 `i` 个点。
    pub fn get(&self, i: usize) -> Option<(f32, f32)> {
        if i >= self.len {
            return None;
        }
        // 最旧的点位置：若未满则是 0，若已满则是 next
        let start = if self.len < N { 0 } else { self.next };
        Some(self.points[(start + i) % N])
    }

    /// 迭代「最旧 → 最新」。
    pub fn iter(&self) -> impl Iterator<Item = (f32, f32)> + '_ {
        (0..self.len).filter_map(|i| self.get(i))
    }
}

// ============================================================================
// 条形与文字几何
// ============================================================================

/// 把 0–1 的比例转成条形的像素长度。
pub fn bar_length(fraction: f32, total_px: f32) -> f32 {
    let f = if fraction.is_nan() {
        0.0
    } else {
        fraction.clamp(0.0, 1.0)
    };
    f * total_px.max(0.0)
}

/// 后轴滑移的显示上限（超过此值条就满了）。
///
/// 与参考实现一致：1.70。
/// 调低会让条增长更快（更灵敏），调高会更"稳"。
pub const REAR_SLIP_DISPLAY_MAX: f32 = 1.70;

/// 后轴滑移 → 0–1 的条比例。
pub fn slip_fraction(slip: f32) -> f32 {
    if slip.is_nan() {
        return 0.0;
    }
    (slip.abs() / REAR_SLIP_DISPLAY_MAX).clamp(0.0, 1.0)
}

/// 简单的矩形（左上角 + 尺寸）。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    /// 左边界。
    pub x: f32,
    /// 上边界。
    pub y: f32,
    /// 宽度。
    pub w: f32,
    /// 高度。
    pub h: f32,
}

impl Rect {
    /// 构造。
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// 右边界。
    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    /// 下边界。
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    /// 中心点。
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w * 0.5, self.y + self.h * 0.5)
    }

    /// 是否包含某点（用于拖拽命中的命中测试）。
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.right() && py >= self.y && py < self.bottom()
    }

    /// 是否与另一个矩形相交（用于脏区合并）。
    pub fn intersects(&self, other: &Rect) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }

    /// 求并集（用于把多个脏区合并成一个）。
    pub fn union(&self, other: &Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let r = self.right().max(other.right());
        let b = self.bottom().max(other.bottom());
        Rect::new(x, y, r - x, b - y)
    }

    /// 向外扩张 `pad` 像素（用于给辉光留出溢出空间）。
    pub fn inflate(&self, pad: f32) -> Rect {
        Rect::new(
            self.x - pad,
            self.y - pad,
            self.w + pad * 2.0,
            self.h + pad * 2.0,
        )
    }

    /// 与画布求交，保证不越界。
    pub fn clamp_to(&self, canvas: &Rect) -> Rect {
        let x = self.x.max(canvas.x);
        let y = self.y.max(canvas.y);
        let r = self.right().min(canvas.right());
        let b = self.bottom().min(canvas.bottom());
        Rect::new(x, y, (r - x).max(0.0), (b - y).max(0.0))
    }
}

/// 脏区累积器：把一帧内所有变动的矩形合并成尽量少的重绘区域。
///
/// # 为什么需要它
///
/// 参考实现每帧无条件重绘**整块屏幕**（且是 CPU 软件光栅化），
/// 静止时也照样 62.5 fps 全屏重绘 —— 这是它最大的资源问题。
///
/// 本实现按「组件」收集脏区：每帧只有数值变化的组件才登记脏区，
/// 静止时没有任何脏区，于是**完全不重绘**。
///
/// 为了简单与可预测，这里不做复杂的区域合并算法，只保留一个
/// **包围盒**：把本帧所有脏区并成一个大矩形。代价是当两个相隔很远的
/// 组件同时变化时会多画中间空白，收益是实现简单且绝不会漏画。
#[derive(Debug, Clone, Default)]
pub struct DirtyRegion {
    bbox: Option<Rect>,
    /// 本帧被标记为脏的组件数量（用于统计与测试）。
    marks: usize,
}

impl DirtyRegion {
    /// 新建空的脏区。
    pub const fn new() -> Self {
        Self {
            bbox: None,
            marks: 0,
        }
    }

    /// 标记一个矩形为脏。
    pub fn mark(&mut self, r: Rect) {
        if r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        self.marks += 1;
        self.bbox = Some(match self.bbox {
            Some(b) => b.union(&r),
            None => r,
        });
    }

    /// 标记一个点附近 `radius` 范围。
    pub fn mark_point(&mut self, x: f32, y: f32, radius: f32) {
        self.mark(Rect::new(
            x - radius,
            y - radius,
            radius * 2.0,
            radius * 2.0,
        ));
    }

    /// 当前包围盒（空则返回 `None`）。
    pub fn bbox(&self) -> Option<Rect> {
        self.bbox
    }

    /// 本帧脏标记数量。
    pub fn marks(&self) -> usize {
        self.marks
    }

    /// 是否有任何脏区。
    pub fn is_empty(&self) -> bool {
        self.bbox.is_none()
    }

    /// 取出包围盒并重置（每帧渲染后调用）。
    pub fn take(&mut self) -> Option<Rect> {
        self.marks = 0;
        self.bbox.take()
    }

    /// 只重置不取出。
    pub fn clear(&mut self) {
        self.bbox = None;
        self.marks = 0;
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    #[test]
    fn bgra_打包格式正确() {
        let c = Bgra::new(0x11, 0x22, 0x33, 0xFF);
        assert_eq!(c.r, 0x11);
        assert_eq!(c.g, 0x22);
        assert_eq!(c.b, 0x33);
        // ARGB: AARRGGBB
        assert_eq!(c.to_argb(), 0xFF112233);
        // COLORREF: 0x00BBGGRR
        assert_eq!(c.to_colorref(), 0x00332211);
    }

    #[test]
    fn 主题颜色与文档中的十六进制值一致() {
        let t = Theme::street();
        // #55DFFF
        assert_eq!(
            (t.angle_left_ice.r, t.angle_left_ice.g, t.angle_left_ice.b),
            (0x55, 0xDF, 0xFF)
        );
        // #FF3F9B
        assert_eq!(
            (
                t.angle_right_rose.r,
                t.angle_right_rose.g,
                t.angle_right_rose.b
            ),
            (0xFF, 0x3F, 0x9B)
        );
        // #FFB84A
        assert_eq!(
            (
                t.angle_limit_amber.r,
                t.angle_limit_amber.g,
                t.angle_limit_amber.b
            ),
            (0xFF, 0xB8, 0x4A)
        );
        // #FF4A3D
        assert_eq!(
            (
                t.angle_danger_orange.r,
                t.angle_danger_orange.g,
                t.angle_danger_orange.b
            ),
            (0xFF, 0x4A, 0x3D)
        );
        // #05080D
        assert_eq!(
            (t.charcoal.r, t.charcoal.g, t.charcoal.b),
            (0x05, 0x08, 0x0D)
        );
    }

    #[test]
    fn 仪表零度在正上方() {
        // 0 度 → 数学角 -90° → (cos, sin) = (0, -1) → 圆心正上方
        let (x, y) = gauge_point(100.0, 100.0, 50.0, 0.0);
        assert!((x - 100.0).abs() < 1e-4, "x 应为圆心 x, 实际 {x}");
        assert!((y - 50.0).abs() < 1e-3, "y 应为圆心上方 50, 实际 {y}");
    }

    #[test]
    fn 仪表九十度在正右方() {
        let (x, y) = gauge_point(100.0, 100.0, 50.0, 90.0);
        assert!((x - 150.0).abs() < 1e-3, "x 应为圆心右方 50, 实际 {x}");
        assert!((y - 100.0).abs() < 1e-4, "y 应为圆心 y, 实际 {y}");
    }

    #[test]
    fn 仪表负九十度在正左方() {
        let (x, y) = gauge_point(100.0, 100.0, 50.0, -90.0);
        assert!((x - 50.0).abs() < 1e-3, "x 应为圆心左方 50, 实际 {x}");
        assert!((y - 100.0).abs() < 1e-4);
    }

    #[test]
    fn 漂移角映射到仪表后不越界() {
        for a in [-200.0f32, -90.0, -45.0, 0.0, 45.0, 90.0, 200.0] {
            let g = drift_angle_to_gauge(a);
            assert!(
                (-SWEEP_HALF_DEG..=SWEEP_HALF_DEG).contains(&g),
                "输入 {a} 得到越界仪表角 {g}"
            );
        }
        // NaN 兜底为 0
        assert_eq!(drift_angle_to_gauge(f32::NAN), 0.0);
    }

    #[test]
    fn 弧线折线点数与端点正确() {
        // 仪表角度约定：0° 正上方、-90° 正左、+90° 正右。
        // 因此从 -90° 扫 180° 的弧：起点正左 → 中点正上 → 终点正右。
        let arc = ArcSpec {
            start_deg: -90.0,
            sweep_deg: 180.0,
            radius: 50.0,
            width: 4.0,
        };
        let mut buf = [(0.0f32, 0.0f32); 65];
        let n = arc.polyline(100.0, 100.0, 64, &mut buf);
        assert_eq!(n, 65);

        // 起点：-90° → 正左 → (cx - r, cy)
        assert!((buf[0].0 - 50.0).abs() < 1e-3, "起点 x={}", buf[0].0);
        assert!((buf[0].1 - 100.0).abs() < 1e-3, "起点 y={}", buf[0].1);

        // 中点：0° → 正上 → (cx, cy - r)
        assert!((buf[32].0 - 100.0).abs() < 1e-3, "中点 x={}", buf[32].0);
        assert!(
            (buf[32].1 - 50.0).abs() < 1e-3,
            "中点应在圆心正上方, 实际 y={}",
            buf[32].1
        );

        // 终点：+90° → 正右 → (cx + r, cy)
        assert!((buf[64].0 - 150.0).abs() < 1e-3, "终点 x={}", buf[64].0);
        assert!((buf[64].1 - 100.0).abs() < 1e-3, "终点 y={}", buf[64].1);
    }

    #[test]
    fn 弧线各点都落在给定半径上() {
        // 折线只是采样圆弧，每个采样点距圆心都应等于半径。
        let arc = ArcSpec {
            start_deg: -90.0,
            sweep_deg: 180.0,
            radius: 50.0,
            width: 4.0,
        };
        let mut buf = [(0.0f32, 0.0f32); 33];
        let n = arc.polyline(100.0, 100.0, 32, &mut buf);
        for (i, (x, y)) in buf.iter().enumerate().take(n) {
            let d = ((x - 100.0).powi(2) + (y - 100.0).powi(2)).sqrt();
            assert!((d - 50.0).abs() < 1e-2, "第 {i} 点距圆心 {d}，应为 50");
        }
    }

    #[test]
    fn 弧线比例构造夹紧到0到1() {
        let a = ArcSpec::from_fraction(-90.0, 90.0, 0.5, 10.0, 2.0);
        assert_eq!(a.sweep_deg, 90.0);
        assert_eq!(a.end_deg(), 0.0);

        // 超范围输入被夹紧
        let b = ArcSpec::from_fraction(-90.0, 90.0, 5.0, 10.0, 2.0);
        assert_eq!(b.sweep_deg, 180.0);
        let c = ArcSpec::from_fraction(-90.0, 90.0, -3.0, 10.0, 2.0);
        assert_eq!(c.sweep_deg, 0.0);
        let d = ArcSpec::from_fraction(-90.0, 90.0, f32::NAN, 10.0, 2.0);
        assert_eq!(d.sweep_deg, 0.0);
    }

    #[test]
    fn 切向与法向相互垂直且为单位矢量() {
        let arc = ArcSpec {
            start_deg: -90.0,
            sweep_deg: 180.0,
            radius: 40.0,
            width: 3.0,
        };
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            let ((tx, ty), (nx, ny)) = arc.tangent_normal(t);
            // 单位长度
            assert!(((tx * tx + ty * ty).sqrt() - 1.0).abs() < 1e-4);
            assert!(((nx * nx + ny * ny).sqrt() - 1.0).abs() < 1e-4);
            // 相互垂直
            assert!((tx * nx + ty * ny).abs() < 1e-4, "t={t} 切法向不垂直");
        }
    }

    #[test]
    fn 刻度角度均匀分布且包含两端() {
        let mut buf = [0.0f32; 5];
        let n = tick_angles(-90.0, 90.0, 5, &mut buf);
        assert_eq!(n, 5);
        assert_eq!(buf[0], -90.0);
        assert_eq!(buf[4], 90.0);
        assert!((buf[2] - 0.0).abs() < 1e-6);
        // 等距
        for i in 1..5 {
            let d = buf[i] - buf[i - 1];
            assert!((d - 45.0).abs() < 1e-4, "间距 {d} 应为 45");
        }

        // 只有一个刻度时取起点
        let mut one = [0.0f32; 1];
        assert_eq!(tick_angles(-90.0, 90.0, 1, &mut one), 1);
        assert_eq!(one[0], -90.0);

        // 请求数超过缓冲时按缓冲长度截断
        let mut small = [0.0f32; 3];
        assert_eq!(tick_angles(-90.0, 90.0, 99, &mut small), 3);
    }

    #[test]
    fn G点位在正中心时为圆心() {
        let (x, y) = g_point(100.0, 100.0, 50.0, 0.0, 0.0, 2.5);
        assert!((x - 100.0).abs() < 1e-4);
        assert!((y - 100.0).abs() < 1e-4);
    }

    #[test]
    fn G点横向正值为向右() {
        let (x, _) = g_point(100.0, 100.0, 50.0, 1.25, 0.0, 2.5);
        assert!(
            (x - 125.0).abs() < 1e-3,
            "横向 +1.25G / 满量程 2.5 应在右半, 实际 x={x}"
        );
    }

    #[test]
    fn G点纵向正值为向上() {
        // 屏幕 Y 向下，所以「向上」意味着 y 小于圆心
        let (_, y) = g_point(100.0, 100.0, 50.0, 0.0, 1.25, 2.5);
        assert!(y < 100.0, "纵向加速应画在圆心上方, 实际 y={y}");
        assert!((y - 75.0).abs() < 1e-3, "应为圆心上方 25, 实际 {y}");
    }

    #[test]
    fn G点超出量程时被夹到圆上() {
        // 极大的 G 值不应画出圆外
        let (x, y) = g_point(100.0, 100.0, 50.0, 100.0, 100.0, 2.5);
        let d = ((x - 100.0).powi(2) + (y - 100.0).powi(2)).sqrt();
        assert!(d <= 50.0 + 1e-3, "应被夹到圆内, 实际距圆心 {d}");

        // 零量程不应除零
        let (x2, y2) = g_point(100.0, 100.0, 50.0, 1.0, 1.0, 0.0);
        assert!(x2.is_finite() && y2.is_finite());
    }

    #[test]
    fn G表圆环半径比例正确() {
        let (inner, outer) = g_ring_radii(100.0);
        assert!((inner - 62.0).abs() < 1e-3);
        assert!((outer - 92.0).abs() < 1e-3);
        assert!(inner < outer);
    }

    #[test]
    fn 轨迹缓冲满了会覆盖最旧的点() {
        let mut buf: TrailBuf<3> = TrailBuf::new();
        assert!(buf.is_empty());
        assert_eq!(buf.capacity(), 3);

        buf.push((1.0, 1.0));
        buf.push((2.0, 2.0));
        assert_eq!(buf.len(), 2);
        assert_eq!(buf.get(0), Some((1.0, 1.0)));
        assert_eq!(buf.get(1), Some((2.0, 2.0)));
        assert_eq!(buf.get(2), None);

        buf.push((3.0, 3.0));
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.get(2), Some((3.0, 3.0)));

        // 再压一个 → 最旧的 (1,1) 被覆盖
        buf.push((4.0, 4.0));
        assert_eq!(buf.len(), 3, "长度应保持在容量");
        assert_eq!(buf.get(0), Some((2.0, 2.0)), "最旧的应被覆盖");
        assert_eq!(buf.get(1), Some((3.0, 3.0)));
        assert_eq!(buf.get(2), Some((4.0, 4.0)));

        // 顺序应为最旧 → 最新
        let v: Vec<_> = buf.iter().collect();
        assert_eq!(v, vec![(2.0, 2.0), (3.0, 3.0), (4.0, 4.0)]);

        buf.clear();
        assert!(buf.is_empty());
        assert_eq!(buf.get(0), None);
    }

    #[test]
    fn 零容量轨迹缓冲不会panic() {
        let mut buf: TrailBuf<0> = TrailBuf::new();
        buf.push((1.0, 1.0));
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.get(0), None);
        assert_eq!(buf.iter().count(), 0);
    }

    #[test]
    fn 滑移比例换算正确() {
        assert_eq!(slip_fraction(0.0), 0.0);
        assert!((slip_fraction(REAR_SLIP_DISPLAY_MAX) - 1.0).abs() < 1e-6);
        // 超过上限被夹紧
        assert_eq!(slip_fraction(99.0), 1.0);
        // 负值取绝对值（倒车打滑）
        assert!((slip_fraction(-0.85) - 0.5).abs() < 1e-3);
        // NaN 兜底
        assert_eq!(slip_fraction(f32::NAN), 0.0);
    }

    #[test]
    fn 条形长度换算与夹紧() {
        assert_eq!(bar_length(0.5, 100.0), 50.0);
        assert_eq!(bar_length(0.0, 100.0), 0.0);
        assert_eq!(bar_length(1.0, 100.0), 100.0);
        assert_eq!(bar_length(2.0, 100.0), 100.0);
        assert_eq!(bar_length(-1.0, 100.0), 0.0);
        assert_eq!(bar_length(f32::NAN, 100.0), 0.0);
        assert_eq!(bar_length(0.5, -10.0), 0.0, "负长度应归零");
    }

    #[test]
    fn 矩形包含与相交判定正确() {
        let r = Rect::new(10.0, 10.0, 100.0, 50.0);
        assert_eq!(r.right(), 110.0);
        assert_eq!(r.bottom(), 60.0);
        assert_eq!(r.center(), (60.0, 35.0));

        // 左上闭合、右下开区间
        assert!(r.contains(10.0, 10.0));
        assert!(r.contains(109.9, 59.9));
        assert!(!r.contains(110.0, 35.0), "右边界应为开区间");
        assert!(!r.contains(60.0, 60.0), "下边界应为开区间");
        assert!(!r.contains(9.9, 35.0));

        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 10.0, 10.0);
        let c = Rect::new(20.0, 20.0, 5.0, 5.0);
        assert!(a.intersects(&b));
        assert!(b.intersects(&a));
        assert!(!a.intersects(&c));
        // 仅边界相接不算相交
        let d = Rect::new(10.0, 0.0, 5.0, 5.0);
        assert!(!a.intersects(&d));

        let u = a.union(&b);
        assert_eq!((u.x, u.y, u.w, u.h), (0.0, 0.0, 15.0, 15.0));
    }

    #[test]
    fn 矩形扩张与裁剪正确() {
        let r = Rect::new(10.0, 10.0, 10.0, 10.0);
        let g = r.inflate(5.0);
        assert_eq!((g.x, g.y, g.w, g.h), (5.0, 5.0, 20.0, 20.0));

        let canvas = Rect::new(0.0, 0.0, 100.0, 100.0);
        let out = Rect::new(-20.0, -20.0, 60.0, 60.0).clamp_to(&canvas);
        assert_eq!((out.x, out.y, out.w, out.h), (0.0, 0.0, 40.0, 40.0));

        // 完全在画布外 → 尺寸归零而不是负数
        let gone = Rect::new(200.0, 200.0, 10.0, 10.0).clamp_to(&canvas);
        assert_eq!(gone.w, 0.0);
        assert_eq!(gone.h, 0.0);
    }

    #[test]
    fn 脏区累积与包围盒() {
        let mut d = DirtyRegion::new();
        assert!(d.is_empty());
        assert_eq!(d.bbox(), None);

        d.mark(Rect::new(10.0, 10.0, 10.0, 10.0));
        d.mark(Rect::new(50.0, 30.0, 10.0, 10.0));
        assert_eq!(d.marks(), 2);
        let b = d.bbox().unwrap();
        assert_eq!((b.x, b.y, b.w, b.h), (10.0, 10.0, 50.0, 30.0));

        // 零尺寸的标记应被忽略
        d.mark(Rect::new(0.0, 0.0, 0.0, 5.0));
        assert_eq!(d.marks(), 2, "零宽脏区不应计入");

        let taken = d.take();
        assert!(taken.is_some());
        assert!(d.is_empty(), "take 之后应重置");
        assert_eq!(d.marks(), 0);
    }

    #[test]
    fn 静止时脏区为空意味着完全不重绘() {
        // 这条性质是本项目相对参考实现最重要的资源优化：
        // 没有任何组件变化时，脏区为空，渲染层可以整帧跳过。
        let mut d = DirtyRegion::new();
        assert!(d.is_empty(), "静止帧不应有任何脏区");
        assert!(d.take().is_none());

        // 只有一个组件变化时，脏区应只包围该组件
        d.mark(Rect::new(500.0, 400.0, 200.0, 100.0));
        let b = d.take().unwrap();
        assert_eq!((b.x, b.y, b.w, b.h), (500.0, 400.0, 200.0, 100.0));
    }

    #[test]
    fn 颜色插值端点正确() {
        let a = Bgra::new(0, 0, 0, 255);
        let b = Bgra::new(255, 255, 255, 255);
        assert_eq!(a.lerp(b, 0), a);
        assert_eq!(a.lerp(b, 255), b);
        let mid = a.lerp(b, 128);
        // 128/255 ≈ 0.502
        assert!((mid.r as i32 - 128).abs() <= 1);
    }

    #[test]
    fn 风格与风险标签都有颜色映射() {
        use fh6_core::drift::{FlowQuality, RiskLabel, Style};
        let t = Theme::street();
        for s in [
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
        ] {
            let c = t.style_color(s);
            assert_eq!(c.a, 255, "风格色应不透明");
        }
        for r in [
            RiskLabel::Safe,
            RiskLabel::Edge,
            RiskLabel::Risk,
            RiskLabel::Max,
        ] {
            assert_eq!(t.risk_color(r).a, 255);
        }
        for q in [
            FlowQuality::Build,
            FlowQuality::Flow,
            FlowQuality::Chase,
            FlowQuality::Smooth,
            FlowQuality::Locked,
        ] {
            assert_eq!(t.flow_color(q).a, 255);
        }
    }
}
