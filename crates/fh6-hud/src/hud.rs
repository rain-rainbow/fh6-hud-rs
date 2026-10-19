//! HUD 绘制层。
//!
//! # 这一层为什么几乎无法单元测试
//!
//! 它做的事是「把像素写到内存里」。正确性标准是**看起来对不对**，
//! 而不是「返回值等于多少」。所以本模块的策略是：
//!
//! 1. 几何计算**全部**委托给 [`crate::gauge`]（那里有 46 个可测的测试）；
//! 2. 本层只负责「按几何结果落笔」；
//! 3. 提供 [`HudPainter::draw_all`]，可由 `--selftest` 离屏渲染成 BMP 供肉眼核对，
//!    **不需要创建窗口、不需要 GPU、不需要连游戏**。
//!
//! # 绘制技术
//!
//! 全部基于 GDI 的基本图元（直线、多边形、矩形、椭圆、文本）。质感靠
//! **多层半透明叠加**实现：
//!
//! * **辉光**：用逐渐变粗、逐渐变淡的描边叠加模拟外发光；
//! * **扫描线**：每 N 行画一条低透明度横向线；
//! * **半调网点**：规则网格上的低透明度圆点；
//! * **渐变描边**：把弧线切成多段，每段用两个端点色插值。
//!
//! 这些技巧全部是公开的通用图形学做法，与任何第三方项目的具体实现无关。

use crate::gauge::{self, ArcSpec, Bgra, Rect, Theme};
use crate::raster::{Rasterizer, TextAlign};
use crate::shared::LinkState;
use fh6_config::settings::Settings;

use fh6_core::drift::{DriftSnapshot, Popup};

/// 按比例衰减一个颜色的 alpha。
///
/// 绘制层里大量「以某个强度画某色」的场景用它，避免每处都手写 `with_alpha` 的
/// 夹紧与 NaN 处理。
#[inline]
fn with_alpha(c: Bgra, a: f32) -> Bgra {
    let a = if a.is_nan() { 0.0 } else { a.clamp(0.0, 1.0) };
    c.with_alpha((c.a as f32 * a) as u8)
}

/// HUD 绘制器。
pub struct HudPainter<'a> {
    raster: Rasterizer<'a>,
    #[allow(dead_code)]
    settings: &'a Settings,
    theme: Theme,
    /// 本地化文本（由 settings.language 决定）。
    labels: crate::i18n::Labels,
}

impl<'a> HudPainter<'a> {
    /// 新建绘制器。
    pub fn new(
        canvas: &'a mut crate::window::Canvas,
        settings: &'a Settings,
        theme: Theme,
    ) -> Self {
        Self {
            raster: Rasterizer::new(canvas),
            settings,
            theme,
            labels: crate::i18n::Labels::new(settings.language),
        }
    }

    /// 画布尺寸。
    fn size(&self) -> (f32, f32) {
        let (w, h) = self.raster.size();
        (w as f32, h as f32)
    }

    /// 应用全局缩放与偏移到一个坐标。
    fn global(&self, x: f32, y: f32) -> (f32, f32) {
        let s = self.settings.global_scale;
        (
            x * s + self.settings.offset_x,
            y * s + self.settings.offset_y,
        )
    }

    /// 全文清空为透明。
    pub fn clear(&mut self) {
        self.raster.clear_canvas();
    }

    // ---- 基础图元 ----
    /// 画一个矩形框（边框）。
    fn stroke_rect(&mut self, r: Rect, color: Bgra, thickness: i32) {
        self.raster.stroke_rect(r, color, thickness);
    }

    /// 画一条线段。
    fn stroke_line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, color: Bgra, thickness: i32) {
        self.raster.line(x0, y0, x1, y1, color, thickness);
    }

    /// 画点列的连线（弧线的实际落笔方式）。
    fn stroke_polyline(&mut self, pts: &[(f32, f32)], color: Bgra, thickness: i32) {
        self.raster.polyline(pts, color, thickness);
    }

    /// 画填充三角形（用于指针）。
    fn fill_triangle(&mut self, p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), color: Bgra) {
        self.raster.fill_triangle(p0, p1, p2, color);
    }

    /// 画一个实心圆点。
    fn fill_circle(&mut self, cx: f32, cy: f32, r: f32, color: Bgra) {
        self.raster.disc(cx, cy, r, color);
    }

    /// 画一个空心圆环。
    fn stroke_ring(&mut self, cx: f32, cy: f32, radius: f32, thickness: f32, color: Bgra) {
        self.raster.ring(cx, cy, radius, thickness, color);
    }

    /// 直接填充一个矩形（调试与自检用，也供面板底衬调用）。
    pub fn debug_fill_rect(&mut self, r: Rect, color: Bgra) {
        self.raster.fill_rect(r, color);
    }

    /// 在给定矩形内画横向扫描线。
    pub fn scanlines(&mut self, r: Rect, color: Bgra, alpha: f32, spacing: i32) {
        self.raster.scanlines(r, color, alpha, spacing);
    }

    /// 在给定矩形内画半调网点。
    pub fn halftone(&mut self, r: Rect, color: Bgra, alpha: f32, spacing: i32, dot: f32) {
        self.raster.halftone(r, color, alpha, spacing, dot);
    }

    /// 画多层辉光：由粗到细、由淡到浓地叠同一条折线。
    ///
    /// 这是模拟外发光的通用做法：没有模糊滤镜，就用多层描边近似。
    fn glow_polyline(
        &mut self,
        pts: &[(f32, f32)],
        color: Bgra,
        base_width: i32,
        layers: u32,
        strength: f32,
    ) {
        if pts.len() < 2 || layers == 0 || strength <= 0.001 {
            return;
        }
        // 由外到内：最外层最粗最淡
        for i in (1..=layers).rev() {
            let t = i as f32 / layers as f32; // 1.0 = 最外
            let width = base_width + (base_width as f32 * 1.6 * t) as i32;
            let a = strength * 0.28 * (1.0 - t) + strength * 0.10;
            self.stroke_polyline(pts, with_alpha(color, a), width);
        }
        // 最内层是实心主体
        self.stroke_polyline(pts, color, base_width);
    }

    // ---- 文本 ----

    /// 画一段文本，返回其宽度。
    #[allow(clippy::too_many_arguments)]
    pub fn text(
        &mut self,
        s: &str,
        x: f32,
        y: f32,
        size: f32,
        color: Bgra,
        bold: bool,
        align: TextAlign,
    ) -> f32 {
        self.raster.text(s, x, y, size, color, bold, align)
    }

    /// 画带描边的文本（提升在复杂背景上的可读性）。
    #[allow(clippy::too_many_arguments)]
    pub fn text_outlined(
        &mut self,
        s: &str,
        x: f32,
        y: f32,
        size: f32,
        color: Bgra,
        outline: Bgra,
        layers: u32,
        bold: bool,
        align: TextAlign,
    ) -> f32 {
        self.raster
            .text_outlined(s, x, y, size, color, outline, layers, bold, align)
    }

    // ---- 主仪表：漂移角弧 ----

    /// 画主角度表：底弧 + 刻度 + 数值弧 + 指针 + 中心读数。
    ///
    /// 需要 [`HudFrame`] 而不是单独的 [`DriftSnapshot`]，因为中心读数还要用到
    /// 速度与挡位 —— 它们属于 `fh6_core::Derived` 而不属于漂移判定结果。
    pub fn angle_gauge(&mut self, frame: &HudFrame, link: LinkState) {
        let snap = &frame.drift;
        let mut gear_buf = [0u8; 4];
        let p = &self.settings.angle;
        let (cx, cy) = self.global(p.cx, p.cy);
        let radius = p.radius * self.settings.global_scale;
        let width = (p.arc_width * self.settings.global_scale).max(1.0) as i32;
        let segments = self.settings.perf.arc_segments.max(8) as usize;

        // 弧度复用一块栈上缓冲，避免每帧分配
        let mut pts = vec![(0.0f32, 0.0f32); segments + 1];

        // ---- 1. 底弧（整条半圆的淡色轨道）----
        let track = ArcSpec {
            start_deg: -gauge::SWEEP_HALF_DEG,
            sweep_deg: gauge::SWEEP_HALF_DEG * 2.0,
            radius,
            width: width as f32,
        };
        let n = track.polyline(cx, cy, segments, &mut pts);
        self.stroke_polyline(&pts[..n], with_alpha(self.theme.text_dim, 0.22), width);

        // ---- 2. 刻度 ----
        if p.tick_count > 1 {
            let mut angles = vec![0.0f32; p.tick_count as usize];
            let count = gauge::tick_angles(
                -gauge::SWEEP_HALF_DEG,
                gauge::SWEEP_HALF_DEG,
                p.tick_count as usize,
                &mut angles,
            );
            let tick_len = p.tick_length * self.settings.global_scale;
            for (i, deg) in angles.iter().enumerate().take(count) {
                // 主要刻度（每 2 条取一条）更长更亮
                let major = i % 2 == 0;
                let len = if major { tick_len } else { tick_len * 0.55 };
                let a = if major { 0.75 } else { 0.35 };
                let (x0, y0) = gauge::gauge_point(cx, cy, radius - width as f32 * 0.5 - len, *deg);
                let (x1, y1) = gauge::gauge_point(cx, cy, radius - width as f32 * 0.5 - 2.0, *deg);
                self.stroke_line(
                    x0,
                    y0,
                    x1,
                    y1,
                    with_alpha(self.theme.text_white, a),
                    if major { 2 } else { 1 },
                );
            }
        }

        // ---- 3. 数值弧：从 0 度向漂移方向扫出 ----
        // 用「颜色随风险变化」而不是固定配色：SAFE 冷蓝、EDGE 薄荷、
        // RISK 琥珀、MAX 橙红，与风险标签的颜色保持一致，视线不用重新学习。
        let value_color = self.theme.risk_color(snap.risk_label);
        let frac = (snap.display_angle.abs() / gauge::SWEEP_HALF_DEG).clamp(0.0, 1.0);
        if frac > 0.001 {
            let sweep = gauge::drift_angle_to_gauge(snap.display_angle);
            // 弧从 0°（正上方）向漂移方向扫出。sweep 的符号由
            // drift_angle_to_gauge 决定（正 = 顺时针 = 右漂移），
            // 因此正负两种情况用同一个 ArcSpec 即可，不需要分支。
            let arc = ArcSpec {
                start_deg: 0.0,
                sweep_deg: sweep,
                radius,
                width: width as f32,
            };
            let n = arc.polyline(cx, cy, segments, &mut pts);
            let layers = p.glow_layers.round() as u32;
            self.glow_polyline(&pts[..n], value_color, width, layers, p.glow_strength);
        }

        // ---- 4. 指针（切向定位的三角）----
        //
        // 指针不是一条线而是一个小三角，底边沿弧的**切向**展开，
        // 尖端指向当前角度。切向由 gauge::ArcSpec::tangent_normal 给出，
        // 这里直接用同一套角度→坐标的约定，不自己推三角函数。
        let needle_deg = gauge::drift_angle_to_gauge(snap.display_angle);
        let needle_r = radius - width as f32 * 0.9;
        let tip = gauge::gauge_point(cx, cy, needle_r, needle_deg);
        let tip_len = p.needle_length.clamp(0.1, 1.0) * radius * 0.34;
        let base_pt = gauge::gauge_point(cx, cy, needle_r - tip_len, needle_deg);

        // 切向单位矢量：把角度加 90° 得到方向（与 gauge_deg_to_rad 的约定一致）
        let tangent_deg = needle_deg + 90.0;
        let trad = gauge::gauge_deg_to_rad(tangent_deg);
        let tan = (trad.cos(), trad.sin());
        let half = (width as f32 * 0.45).max(3.0);
        let b0 = (base_pt.0 - tan.0 * half, base_pt.1 - tan.1 * half);
        let b1 = (base_pt.0 + tan.0 * half, base_pt.1 + tan.1 * half);
        self.fill_triangle(tip, b0, b1, self.theme.text_white);

        // 中心轴点
        self.fill_circle(cx, cy, (width as f32 * 0.38).max(3.0), self.theme.charcoal);
        self.fill_circle(
            cx,
            cy,
            (width as f32 * 0.22).max(1.5),
            with_alpha(self.theme.text_white, 0.9),
        );

        // ---- 5. 中心读数：角度 / 速度 / 挡位 ----
        let big = (p.radius * 0.30).max(14.0) * self.settings.global_scale;
        let angle_text = format!("{:+.0}", snap.display_angle);
        self.text_outlined(
            &angle_text,
            cx,
            cy - big * 0.55,
            big,
            self.theme.text_white,
            self.theme.charcoal.with_alpha(200),
            2,
            true,
            TextAlign::Center,
        );
        // 速度（单位在下方单独标出，避免数字太长）
        let speed_text = format!("{:.0}", frame.core.derived.speed_kph);
        self.text(
            &speed_text,
            cx,
            cy + big * 0.55,
            big * 0.60,
            value_color,
            true,
            TextAlign::Center,
        );
        self.text(
            self.labels.ui(crate::i18n::UiText::SpeedUnit),
            cx,
            cy + big * 1.22,
            big * 0.30,
            with_alpha(self.theme.text_dim, 0.9),
            false,
            TextAlign::Center,
        );
        // 挡位（0 显示为 R）
        let gear = frame.core.derived.gear_display(&mut gear_buf);
        self.text(
            gear,
            cx + radius * 0.62,
            cy - big * 0.35,
            big * 0.95,
            self.theme.accent_amber,
            true,
            TextAlign::Center,
        );

        // 反打量环（有反打时在中心外圈加一段弧，直观显示反打强度）
        if snap.countering && snap.counter_pct > 1.0 {
            let cr = radius * 0.42;
            let arc = ArcSpec::from_fraction(-90.0, 90.0, snap.counter_pct / 100.0, cr, 5.0);
            let mut cpts = vec![(0.0f32, 0.0f32); 33];
            let cn = arc.polyline(cx, cy, 32, &mut cpts);
            self.stroke_polyline(&cpts[..cn], self.theme.accent_mint, 5);
        }

        // ---- 6. 连接状态 ----
        if link != LinkState::Live {
            let c = match link {
                LinkState::NoData => self.theme.state_danger,
                LinkState::Stale => self.theme.state_warn,
                LinkState::Live => self.theme.state_good,
            };
            self.text(
                self.labels.link(link),
                cx,
                cy + radius * 0.66,
                (p.radius * 0.14).max(11.0) * self.settings.global_scale,
                c,
                true,
                TextAlign::Center,
            );
        }
    }

    /// 画 G 表：圆环 + 网格 + 轨迹 + 当前点。
    pub fn g_meter(&mut self, framex: &HudFrame, trail: &[(f32, f32)]) {
        let snap = &framex.drift;
        let p = &self.settings.g_meter;
        let (cx, cy) = self.global(p.cx, p.cy);
        let radius = p.radius * self.settings.global_scale;
        let (inner, outer) = gauge::g_ring_radii(radius);

        // 圆环底色：透明度由配置决定（默认 0.12，偏透明）。
        // 用 ring 原语而不是多边形逼近 —— 它带边缘抗锯齿，圆边不会有锯齿。
        let _ = inner;
        let ring_alpha = (255.0 * p.backdrop_alpha.clamp(0.0, 1.0)) as u8;
        if ring_alpha > 0 {
            self.stroke_ring(
                cx,
                cy,
                (outer + inner) * 0.5,
                outer - inner,
                self.theme.charcoal.with_alpha(ring_alpha),
            );
        }
        // 外圈描边，加强轮廓
        self.stroke_ring(cx, cy, outer, 1.5, with_alpha(self.theme.text_dim, 0.5));
        let segments = 72usize;

        // 十字网格
        if p.show_grid {
            let a = self.theme.text_dim.with_alpha(60);
            self.stroke_line(cx - outer, cy, cx + outer, cy, a, 1);
            self.stroke_line(cx, cy - outer, cx, cy + outer, a, 1);
            // 参考圈（半量程）
            let mid = outer * 0.5;
            let mut circle: Vec<(f32, f32)> = Vec::with_capacity(segments + 1);
            for i in 0..=segments {
                let ang = i as f32 / segments as f32 * std::f32::consts::TAU;
                circle.push((cx + mid * ang.cos(), cy + mid * ang.sin()));
            }
            self.stroke_polyline(&circle, with_alpha(self.theme.text_dim, 0.18), 1);
        }

        // 轨迹
        if p.show_trail && trail.len() >= 2 {
            let mut pts = Vec::with_capacity(trail.len());
            for (i, (gl, glong)) in trail.iter().enumerate() {
                let (x, y) = gauge::g_point(cx, cy, radius, *gl, *glong, p.max_g);
                let _ = i;
                pts.push((x, y));
            }
            // 由旧到新逐渐变亮，形成"尾迹"感
            for i in 1..pts.len() {
                let t = i as f32 / pts.len() as f32;
                let a = p.trail_alpha * t * 0.9;
                self.stroke_line(
                    pts[i - 1].0,
                    pts[i - 1].1,
                    pts[i].0,
                    pts[i].1,
                    with_alpha(self.theme.accent_mint, a),
                    2,
                );
            }
        }

        // 当前点
        let (px, py) = gauge::g_point(cx, cy, radius, snap.g_lateral, snap.g_longitudinal, p.max_g);
        let col = if snap.g_lateral.abs() + snap.g_longitudinal.abs() > p.max_g * 0.85 {
            self.theme.state_danger
        } else {
            self.theme.accent_mint
        };
        self.fill_circle(
            px,
            py,
            p.dot_radius.max(1.0) * self.settings.global_scale,
            col,
        );
        self.fill_circle(px, py, p.dot_radius.max(1.0) * 0.4, self.theme.text_white);

        // 标题与数值
        let title_size = (p.radius * 0.13).max(9.0);
        self.text(
            self.labels.ui(crate::i18n::UiText::Gmeter),
            cx - outer,
            cy - outer - title_size * 1.4,
            title_size,
            with_alpha(self.theme.text_white, 0.8),
            true,
            TextAlign::Left,
        );
        let txt = format!("{:.2} / {:.2}", snap.g_lateral, snap.g_longitudinal);
        self.text(
            &txt,
            cx,
            cy + outer + title_size * 0.4,
            title_size,
            self.theme.text_white,
            false,
            TextAlign::Center,
        );
    }

    /// 用给定颜色填充多边形（GDI 的 `Polygon`）。
    pub fn bar_panel(
        &mut self,
        p: &fh6_config::settings::BarPanelParams,
        title: &str,
        bars: &[BarItem],
    ) {
        let (x, y) = self.global(p.x, p.y);
        let wdt = p.w * self.settings.global_scale;
        // 面板高度：标题一行 + 每个条形（标签行 + 条形行）。
        // 不能简单沿用配置里的 h，否则行数多时标签会压到条形上。
        let title_sz = p.title_size * self.settings.global_scale;
        let bar_th = (p.bar_thickness * self.settings.global_scale).max(3.0);
        // 注意：字形实际高度约为名义字号的 1.3 倍（含内部行距），
        // 所以行高必须在 label_sz 之外再留出这段空间，否则标签会压到条形上。
        let label_sz = (title_sz * 0.72).max(6.0);
        // 中文字形比拉丁大写字母高，行高需要同步放大，否则标签会压到条形上。
        let glyph_h = label_sz * 1.35 * self.labels.glyph_factor();
        let row_h = glyph_h + bar_th + 4.0 * self.labels.line_factor();
        let needed = title_sz * 1.5 + row_h * bars.len() as f32 + 10.0;
        let hgt = (p.h * self.settings.global_scale).max(needed);
        let panel = Rect::new(x, y, wdt, hgt);

        // 面板底：半透明炭黑。软件光栅化支持 alpha，这也是不能用 GDI 的原因之一。
        self.raster.fill_rect(
            panel,
            self.theme
                .charcoal
                .with_alpha((p.panel_opacity * 255.0) as u8),
        );

        if p.scanline_alpha > 0.001 {
            self.scanlines(panel, self.theme.text_white, p.scanline_alpha * 0.5, 3);
        }

        if p.show_frame {
            self.stroke_rect(panel, with_alpha(self.theme.text_dim, 0.55), 1);
            // 四角括号：比整圈边框更有"仪器"感
            let c = 12.0;
            let col = with_alpha(self.theme.accent_mint, 0.8);
            let corners = [
                (panel.x, panel.y, 1.0, 1.0),
                (panel.right(), panel.y, -1.0, 1.0),
                (panel.x, panel.bottom(), 1.0, -1.0),
                (panel.right(), panel.bottom(), -1.0, -1.0),
            ];
            for (px, py, dx, dy) in corners {
                self.stroke_line(px, py, px + c * dx, py, col, 2);
                self.stroke_line(px, py, px, py + c * dy, col, 2);
            }
        }

        // 标题
        let title_size = p.title_size * self.settings.global_scale;
        self.text(
            title,
            panel.x + 8.0,
            panel.y + 4.0,
            title_size,
            with_alpha(self.theme.text_white, 0.85),
            true,
            TextAlign::Left,
        );

        // 条形
        let bar_len = p.bar_length * self.settings.global_scale;
        let thick = bar_th;
        let label_size = label_sz;
        let value_size = label_sz;
        let mut by = panel.y + title_sz * 1.6 + 8.0;
        for item in bars {
            let frac = if item.fraction.is_nan() {
                0.0
            } else {
                item.fraction.clamp(0.0, 1.0)
            };
            // 轨道
            self.raster.fill_rect(
                Rect::new(panel.x + 8.0, by, bar_len, thick),
                Bgra::new(255, 255, 255, 26),
            );
            // 填充
            let filled = gauge::bar_length(frac, bar_len);
            if filled >= 0.5 {
                self.raster.fill_rect(
                    Rect::new(panel.x + 8.0, by, filled, thick),
                    item.color.with_alpha(item.color.a),
                );
                // 前端高光
                self.raster.fill_rect(
                    Rect::new(panel.x + 8.0 + filled - 1.5, by, 1.5, thick),
                    self.theme.text_white.with_alpha(220),
                );
            }
            // 标签
            // 标签画在条形正上方，间距由 row_h 保证不重叠
            // label_y 是文本绘制锚点；GDI 字形实际从锚点下方约 0.5 个字高处开始，
            // 实测（examples/diag_layout）文字占位比锚点低约 0.48 个字号，这里留出余量。
            let label_y = by - glyph_h - 2.0;
            self.text(
                &item.label,
                panel.x + 8.0,
                label_y,
                label_size,
                with_alpha(self.theme.text_white, 0.75),
                false,
                TextAlign::Left,
            );
            if p.show_value {
                self.text(
                    &item.value,
                    panel.x + 8.0 + bar_len,
                    label_y,
                    value_size,
                    item.color,
                    true,
                    TextAlign::Right,
                );
            }
            by += row_h;
        }
    }

    /// 画操纵输入面板：油门 / 刹车 / 离合 / 手刹 / 方向盘。
    pub fn input_panel(&mut self, frame: &HudFrame) {
        let d = &frame.core.derived;
        let dir = if d.steer_pct < -0.5 {
            self.labels.ui(crate::i18n::UiText::SteerLeft)
        } else if d.steer_pct > 0.5 {
            self.labels.ui(crate::i18n::UiText::SteerRight)
        } else {
            "-"
        };
        let bars = [
            BarItem {
                label: self.labels.ui(crate::i18n::UiText::Throttle).to_string(),
                fraction: d.throttle_pct / 100.0,
                color: self.theme.state_good,
                value: format!("{:.0}%", d.throttle_pct),
            },
            BarItem {
                label: self.labels.ui(crate::i18n::UiText::Brake).to_string(),
                fraction: d.brake_pct / 100.0,
                color: self.theme.state_danger,
                value: format!("{:.0}%", d.brake_pct),
            },
            BarItem {
                label: self.labels.ui(crate::i18n::UiText::Clutch).to_string(),
                fraction: d.clutch_pct / 100.0,
                color: self.theme.accent_amber,
                value: format!("{:.0}%", d.clutch_pct),
            },
            BarItem {
                label: self.labels.ui(crate::i18n::UiText::Handbrake).to_string(),
                fraction: d.handbrake_pct / 100.0,
                color: self.theme.angle_right_rose,
                value: format!("{:.0}%", d.handbrake_pct),
            },
            BarItem {
                label: format!("{} ({dir})", self.labels.ui(crate::i18n::UiText::Steer)),
                // 方向盘是 -100..100，条形显示幅值，方向看上面的 L/R
                fraction: (d.steer_pct.abs() / 100.0).min(1.0),
                color: self.theme.angle_left_ice,
                value: format!("{:+.0}", d.steer_pct),
            },
        ];
        let p = self.settings.input_panel;
        let title = self.labels.ui(crate::i18n::UiText::InputPanel);
        self.bar_panel(&p, title, &bars);
    }

    /// 画后轴滑移面板（漂移强度最直观的指标）。
    pub fn slip_panel(&mut self, frame: &HudFrame) {
        let t = &frame.core.telemetry;
        let d = &frame.core.derived;
        let bars = [
            BarItem {
                label: self.labels.ui(crate::i18n::UiText::RearLeft).to_string(),
                fraction: gauge::slip_fraction(t.wheel_rl.combined_slip),
                color: self.slip_color(t.wheel_rl.combined_slip),
                value: format!("{:.2}", t.wheel_rl.combined_slip),
            },
            BarItem {
                label: self.labels.ui(crate::i18n::UiText::RearRight).to_string(),
                fraction: gauge::slip_fraction(t.wheel_rr.combined_slip),
                color: self.slip_color(t.wheel_rr.combined_slip),
                value: format!("{:.2}", t.wheel_rr.combined_slip),
            },
            BarItem {
                label: self.labels.ui(crate::i18n::UiText::RearAvg).to_string(),
                fraction: gauge::slip_fraction(d.rear_slip),
                color: self.slip_color(d.rear_slip),
                value: format!("{:.2}", d.rear_slip),
            },
            BarItem {
                label: self.labels.ui(crate::i18n::UiText::Rpm).to_string(),
                fraction: d.rpm_ratio,
                color: self.theme.accent_mint,
                value: format!("{:.0}", t.current_engine_rpm),
            },
            BarItem {
                label: self.labels.ui(crate::i18n::UiText::Counter).to_string(),
                fraction: frame.drift.counter_pct / 100.0,
                color: self.theme.accent_amber,
                value: format!("{:.0}", frame.drift.counter_pct),
            },
        ];
        let p = self.settings.slip_panel;
        let title = self.labels.ui(crate::i18n::UiText::DriftPanel);
        self.bar_panel(&p, title, &bars);
    }

    /// 滑移量对应的颜色：越大越危险。
    fn slip_color(&self, slip: f32) -> Bgra {
        let s = gauge::slip_fraction(slip);
        if s > 0.85 {
            self.theme.state_danger
        } else if s > 0.55 {
            self.theme.angle_limit_amber
        } else {
            self.theme.accent_mint
        }
    }

    /// 画事件弹窗。
    pub fn popup(&mut self, frame: &HudFrame) {
        let snap = &frame.drift;
        let Some(popup) = snap.popup.popup else {
            return;
        };
        let p = &self.settings.popup;
        let (cx, cy) = self.global(p.cx, p.cy);
        let is_hero = popup.is_hero();
        let base = match popup {
            Popup::SpinSave => self.theme.accent_amber,
            Popup::BigAngle => self.theme.angle_danger_orange,
            Popup::LongHold => self.theme.accent_mint,
            Popup::FlowLock => self.theme.text_white,
            Popup::DeepLine => self.theme.angle_left_ice,
            Popup::LimitEdge => self.theme.angle_limit_amber,
            Popup::SmokeRun => self.theme.text_dim,
            Popup::Entry => self.theme.accent_mint,
        };
        let size = self.settings.popup.scale
            * self.settings.global_scale
            * 34.0
            * if is_hero { p.hero_scale.max(1.0) } else { 1.0 };

        // 半透明底衬：弹窗颜色多为亮色，直接叠在仪表弧线上会看不清。
        // 用估计宽度而不是真实文本度量 —— 底衬宁可略宽一点，
        // 也不要在渲染前先做一次字形光栅化（那会多一次 GDI 往返）。
        let title = self.labels.popup(popup);
        let est_w = (title.chars().count() as f32) * size * 0.72 + size;
        let est_h = size * 1.45;
        // 底衬透明度由配置决定：默认 0.22，调成 0 就完全不画底衬。
        let backdrop = (255.0 * self.settings.popup.backdrop_alpha.clamp(0.0, 1.0)) as u8;
        if backdrop > 0 {
            self.raster.fill_rect(
                Rect::new(cx - est_w * 0.5, cy - size * 0.28, est_w, est_h),
                self.theme.charcoal.with_alpha(backdrop),
            );
        }

        // 淡出：剩余时间小于 0.25 秒时按比例降低不透明度
        let fade = (snap.popup.ttl / 0.25).clamp(0.0, 1.0);
        let col = with_alpha(base, fade);
        let outline = with_alpha(self.theme.charcoal, fade * 0.95);

        self.text_outlined(
            title,
            cx,
            cy,
            size,
            col,
            outline,
            p.outline_layers,
            true,
            TextAlign::Center,
        );

        // 长按进度条（仅 LONG HOLD 时显示）
        if p.show_hold_meter && popup == Popup::LongHold {
            let w_ = size * 6.0;
            let h_ = 4.0;
            let bx = cx - w_ * 0.5;
            let by = cy + size * 1.25;
            let frac = (snap.hold_seconds / 8.0).clamp(0.0, 1.0);
            self.raster
                .fill_rect(Rect::new(bx, by, w_, h_), Bgra::new(255, 255, 255, 40));
            self.raster.fill_rect(Rect::new(bx, by, w_ * frac, h_), col);
        }
    }

    /// 画右上角的状态栏：FLOW / RISK / 风格 / 状态 + 帧率统计。
    pub fn status_panel(&mut self, frame: &HudFrame) {
        let snap = &frame.drift;
        let fps = frame.fps;
        let uploads = frame.uploads;
        let p = self.settings.flow_panel;
        let (x, y) = self.global(p.x, p.y);
        let wdt = p.w * self.settings.global_scale;
        let lines = 6;
        // 行距必须容纳中文字形（汉字占满字身框，比拉丁大写字母高约 28%）
        let lh = 15.0 * self.settings.global_scale * self.labels.line_factor();
        let hgt = lines as f32 * lh + 14.0;
        let panel = Rect::new(x, y, wdt, hgt);

        self.raster.fill_rect(
            panel,
            self.theme
                .charcoal
                .with_alpha((p.panel_opacity * 255.0) as u8),
        );
        if p.scanline_alpha > 0.001 {
            self.scanlines(panel, self.theme.text_white, p.scanline_alpha * 0.4, 3);
        }
        if p.show_frame {
            self.stroke_rect(panel, with_alpha(self.theme.text_dim, 0.5), 1);
        }

        let ts = p.title_size * self.settings.global_scale;
        let mut ly = panel.y + 6.0;
        let mut line = |me: &mut Self, k: &str, v: &str, c: Bgra| {
            me.text(
                k,
                panel.x + 8.0,
                ly,
                ts,
                with_alpha(me.theme.text_dim, 0.9),
                false,
                TextAlign::Left,
            );
            me.text(v, panel.right() - 8.0, ly, ts, c, true, TextAlign::Right);
            ly += lh;
        };

        line(
            self,
            self.labels.ui(crate::i18n::UiText::FlowRow),
            &format!("{:.0}", snap.flow_pct),
            self.theme.flow_color(snap.flow_quality),
        );
        line(
            self,
            self.labels.ui(crate::i18n::UiText::Quality),
            self.labels.flow_quality(snap.flow_quality),
            self.theme.flow_color(snap.flow_quality),
        );
        line(
            self,
            self.labels.ui(crate::i18n::UiText::Risk),
            &format!("{:.0}", snap.spin_risk),
            self.theme.risk_color(snap.risk_label),
        );
        line(
            self,
            self.labels.ui(crate::i18n::UiText::Level),
            self.labels.risk(snap.risk_label),
            self.theme.risk_color(snap.risk_label),
        );
        line(
            self,
            self.labels.ui(crate::i18n::UiText::Style),
            self.labels.style(snap.style),
            self.theme.style_color(snap.style),
        );
        line(
            self,
            self.labels.ui(crate::i18n::UiText::State),
            &format!(
                "{} {}",
                self.labels.drift_state(snap.drift_state),
                snap.holding_drift as u8
            ),
            self.theme.text_white,
        );

        // 右下角小字：帧率与上传次数（用于核对脏区重绘是否生效）
        let small = (ts * 0.75).max(7.0);
        self.text(
            &format!("{fps:.0} fps  up{uploads}"),
            panel.x + 8.0,
            panel.bottom() + 3.0,
            small,
            with_alpha(self.theme.text_dim, 0.8),
            false,
            TextAlign::Left,
        );
    }

    /// 画一个用于自检的模式图（不需要遥测数据）。
    ///
    /// `--selftest` 用它离屏渲染，供肉眼核对绘制是否正确。
    pub fn draw_test_pattern(&mut self) {
        let (cw, ch) = self.size();
        // 背景网格，便于确认坐标方向与缩放
        let grid = self.theme.text_dim.with_alpha(40);
        let mut x = 0.0;
        while x < cw {
            self.raster.fill_rect(Rect::new(x, 0.0, 1.0, ch), grid);
            x += 100.0;
        }
        let mut y = 0.0;
        while y < ch {
            self.raster.fill_rect(Rect::new(0.0, y, cw, 1.0), grid);
            y += 100.0;
        }

        // 坐标轴方向验证：左上角标原点，向右/向下各画一条亮线
        self.stroke_line(0.0, 0.0, 120.0, 0.0, self.theme.state_danger, 3);
        self.stroke_line(0.0, 0.0, 0.0, 120.0, self.theme.state_good, 3);
        self.text(
            "原点 (0,0)",
            6.0,
            6.0,
            14.0,
            self.theme.text_white,
            true,
            TextAlign::Left,
        );
        self.text(
            "+X 右",
            126.0,
            0.0,
            14.0,
            self.theme.state_danger,
            true,
            TextAlign::Left,
        );
        self.text(
            "+Y 下",
            4.0,
            124.0,
            14.0,
            self.theme.state_good,
            true,
            TextAlign::Left,
        );

        // 仪表零度应在正上方 → 画一条参考线确认角度约定
        let p = &self.settings.angle;
        let (cx, cy) = self.global(p.cx, p.cy);
        let radius = p.radius * self.settings.global_scale;
        let top = gauge::gauge_point(cx, cy, radius, 0.0);
        let right = gauge::gauge_point(cx, cy, radius, 90.0);
        let left = gauge::gauge_point(cx, cy, radius, -90.0);
        self.stroke_line(cx, cy, top.0, top.1, self.theme.accent_amber, 2);
        self.stroke_line(cx, cy, right.0, right.1, self.theme.accent_mint, 2);
        self.stroke_line(cx, cy, left.0, left.1, self.theme.angle_left_ice, 2);
        self.text(
            "0°",
            top.0,
            top.1 - 20.0,
            16.0,
            self.theme.accent_amber,
            true,
            TextAlign::Center,
        );
        self.text(
            "+90°",
            right.0 + 6.0,
            right.1,
            16.0,
            self.theme.accent_mint,
            true,
            TextAlign::Left,
        );
        self.text(
            "-90°",
            left.0 - 6.0,
            left.1,
            16.0,
            self.theme.angle_left_ice,
            true,
            TextAlign::Right,
        );
    }
}

/// 条形面板中的一项。
#[derive(Debug, Clone)]
pub struct BarItem {
    /// 标签（左侧）。
    pub label: String,
    /// 0..1 的比例。
    pub fraction: f32,
    /// 颜色。
    pub color: Bgra,
    /// 右侧数值文本。
    pub value: String,
}

/// HUD 使用的字体。
///
/// `Bahnschrift` 是 Windows 10 起自带的 DIN 风格字体，窄而清晰、适合仪表读数。
/// 若系统没有它会由 GDI 自动替换，不会失败。
pub const HUD_FONT: &str = "Bahnschrift";

/// 一帧要画到屏幕上的全部内容。
///
/// 把「遥测 + 派生量 + 漂移判定结果 + 链接状态 + 统计」打包在一起，
/// 让 [`HudPainter`] 的每个绘制方法都能拿到所需的一切，
/// 而不必让调用方逐个传参（那样很容易在新增面板时漏传）。
#[derive(Debug, Clone, Copy)]
pub struct HudFrame {
    /// 原始遥测 + 派生量（速度、挡位、输入百分比等在这里）。
    pub core: fh6_core::Frame,
    /// 漂移判定结果（FLOW / RISK / 风格 / 状态 / 弹窗）。
    pub drift: DriftSnapshot,
    /// 数据链接状态。
    pub link: LinkState,
    /// 当前实测帧率。
    pub fps: f32,
    /// 窗口累计上传次数（用于核对脏区重绘是否生效）。
    pub uploads: u64,
}

impl HudFrame {
    /// 构造。
    pub fn new(
        core: fh6_core::Frame,
        drift: DriftSnapshot,
        link: LinkState,
        fps: f32,
        uploads: u64,
    ) -> Self {
        Self {
            core,
            drift,
            link,
            fps,
            uploads,
        }
    }

    /// 全部内容都画一遍。
    ///
    /// `trail` 是 G 值历史轨迹（由调用方维护，见 [`crate::gauge::TrailBuf`]）。
    pub fn paint(&self, painter: &mut HudPainter<'_>, trail: &[(f32, f32)]) {
        painter.angle_gauge(self, self.link);
        painter.g_meter(self, trail);
        painter.input_panel(self);
        painter.slip_panel(self);
        painter.status_panel(self);
        painter.popup(self);
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use crate::gauge::Bgra;

    #[test]
    fn 绘制器使用的对齐枚举与光栅化器一致() {
        // TextAlign 已统一到 raster 模块；这里确认 hud 导出的就是同一个类型，
        // 避免将来又出现两套枚举导致调用点类型不匹配。
        let a: TextAlign = TextAlign::Center;
        assert_eq!(a, crate::raster::TextAlign::Center);
    }

    #[test]
    fn 透明度合成被夹紧且NaN兜底() {
        let c = Bgra::new(255, 255, 255, 200);
        assert_eq!(with_alpha(c, 0.5).a, 100);
        assert_eq!(with_alpha(c, 1.0).a, 200);
        assert_eq!(with_alpha(c, 0.0).a, 0);
        assert_eq!(with_alpha(c, 5.0).a, 200, "超过 1 应夹紧");
        assert_eq!(with_alpha(c, -1.0).a, 0);
        assert_eq!(with_alpha(c, f32::NAN).a, 0, "NaN 应回退为全透明");
    }

    #[test]
    fn 文本对齐枚举互不相同() {
        assert_ne!(TextAlign::Left, TextAlign::Center);
        assert_ne!(TextAlign::Center, TextAlign::Right);
        assert_ne!(TextAlign::Left, TextAlign::Right);
    }

    #[test]
    fn 条形项可构造() {
        let b = BarItem {
            label: "THR".to_string(),
            fraction: 0.5,
            color: Bgra::rgb(0, 255, 0),
            value: "50%".to_string(),
        };
        assert_eq!(b.label, "THR");
        assert!((b.fraction - 0.5).abs() < 1e-6);
    }

    #[test]
    fn 字体名非空() {
        assert!(!HUD_FONT.is_empty(), "必须配置一个字体名");
        assert!(HUD_FONT.is_ascii(), "字体名用 ASCII，避免区域设置问题");
    }
}
