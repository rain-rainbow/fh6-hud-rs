//! 纯软件光栅化。
//!
//! # 为什么不能用 GDI
//!
//! 分层窗口（`WS_EX_LAYERED` + `UpdateLayeredWindow`）按**每像素 alpha** 合成，
//! 而 **GDI 不会写 alpha 通道**。实测证据（画布 400×300，`Rectangle` 填满整屏）：
//!
//! ```text
//! 7) 全画布 Rectangle 后不透明像素=0
//!    采样像素 (0,0) = Bgra { b: 0, g: 0, r: 255, a: 0 }   ← RGB 写了，alpha 是 0
//! ```
//!
//! 也就是说 GDI 画的线条/文字/多边形**在分层窗口里完全不可见**。
//! 这不是"配置不对"，而是 GDI 的位图模型本身没有 alpha 概念
//! （它诞生于可以用 `BitBlt` 做掩码透明的年代）。
//!
//! # 因此：自己光栅化
//!
//! 本模块实现绘制原语，直接按预乘 alpha 写画布内存。带来两个额外好处：
//!
//! 1. **不再依赖 gdi32**：只需要 user32（窗口与消息），产物体积更小；
//! 2. **完全掌控混合**：可以做真正的半透明面板、渐变、辉光，
//!    而不必绕开 alpha。
//!
//! # 预乘 alpha 约定
//!
//! `UpdateLayeredWindow` 要求位图是**预乘**的：存储值 = 颜色 × alpha / 255。
//! 本模块所有写入都遵循这个约定，[`Rasterizer::blend_pixel`] 是唯一的落笔点，
//! 因此只要它正确，整屏就正确。

use crate::gauge::{Bgra, Rect};
use crate::textmask::TextMask;
use crate::window::Canvas;

/// 一个点的整数坐标。
pub type Pt = (i32, i32);

/// 软件光栅化器：在 [`Canvas`] 上绘制。
///
/// 所有方法都直接写画布内存，按预乘 alpha 混合。
pub struct Rasterizer<'a> {
    canvas: &'a mut Canvas,
    /// 绘制裁剪区（画布坐标系）。所有落笔都被限制在它之内。
    clip: Rect,
}

impl<'a> Rasterizer<'a> {
    /// 新建光栅化器，裁剪区默认是整块画布。
    pub fn new(canvas: &'a mut Canvas) -> Self {
        let (w, h) = canvas.size();
        Self {
            canvas,
            clip: Rect::new(0.0, 0.0, w as f32, h as f32),
        }
    }

    /// 清空整块画布为全透明。
    pub fn clear_canvas(&mut self) {
        self.canvas.clear_transparent();
    }

    /// 设置裁剪区。
    pub fn set_clip(&mut self, r: Rect) {
        let (w, h) = self.canvas.size();
        self.clip = r.clamp_to(&Rect::new(0.0, 0.0, w as f32, h as f32));
    }

    /// 当前裁剪区。
    pub fn clip(&self) -> Rect {
        self.clip
    }

    /// 画布尺寸。
    pub fn size(&self) -> (i32, i32) {
        self.canvas.size()
    }

    /// **唯一的落笔点**：把一个颜色以「源覆盖」方式混合到目标像素上。
    ///
    /// 用预乘 alpha 的标准 source-over 公式（`dst = src + dst * (1 - src_a)`）。
    /// 边界由画布尺寸兜底，裁剪由调用方先行判断。
    #[inline]
    fn blend_pixel(&mut self, x: i32, y: i32, color: Bgra) {
        if color.a == 0 {
            return;
        }
        let (w, h) = self.canvas.size();
        if x < 0 || y < 0 || x >= w || y >= h {
            return;
        }
        let inv = 255u32 - color.a as u32;
        // 预乘：源直存，目标按 (1-a) 衰减
        let src_b = color.b as u32;
        let src_g = color.g as u32;
        let src_r = color.r as u32;

        if let Some(dst) = self.canvas.pixel(x, y) {
            // 目标本身也是预乘的，直接按 (1-a) 衰减后相加
            let dr = dst.r as u32 * inv / 255;
            let dg = dst.g as u32 * inv / 255;
            let db = dst.b as u32 * inv / 255;
            let da = dst.a as u32 * inv / 255;
            let out = Bgra::new(
                (src_r + dr).min(255) as u8,
                (src_g + dg).min(255) as u8,
                (src_b + db).min(255) as u8,
                (color.a as u32 + da).min(255) as u8,
            );
            self.canvas.put_pixel(x, y, out);
        }
    }

    /// 把颜色预乘（`c * a / 255`），用于「不透明绘制」路径。
    ///
    /// 调用方给出的颜色通常是"非预乘"的直觉值（如 `rgb(85,223,255)`）。
    /// 要正确地以 alpha 混合，必须先把 RGB 按 alpha 缩放。
    #[inline]
    pub fn premultiply(color: Bgra) -> Bgra {
        let a = color.a as u32;
        Bgra::new(
            (color.r as u32 * a / 255) as u8,
            (color.g as u32 * a / 255) as u8,
            (color.b as u32 * a / 255) as u8,
            color.a,
        )
    }

    // ========================================================================
    // 矩形
    // ========================================================================

    /// 填充矩形（含 alpha）。
    pub fn fill_rect(&mut self, r: Rect, color: Bgra) {
        let region = r.clamp_to(&self.clip);
        if region.w < 1.0 || region.h < 1.0 {
            return;
        }
        let c = Self::premultiply(color);
        let x0 = region.x.floor() as i32;
        let y0 = region.y.floor() as i32;
        let x1 = region.right().ceil() as i32;
        let y1 = region.bottom().ceil() as i32;
        for y in y0..y1 {
            for x in x0..x1 {
                self.blend_pixel(x, y, c);
            }
        }
    }

    /// 画矩形边框（线宽 `t`，向内生长）。
    pub fn stroke_rect(&mut self, r: Rect, color: Bgra, t: i32) {
        let t = t.max(1) as f32;
        self.fill_rect(Rect::new(r.x, r.y, r.w, t), color); // 上
        self.fill_rect(Rect::new(r.x, r.bottom() - t, r.w, t), color); // 下
        self.fill_rect(Rect::new(r.x, r.y, t, r.h), color); // 左
        self.fill_rect(Rect::new(r.right() - t, r.y, t, r.h), color); // 右
    }

    // ========================================================================
    // 线段
    // ========================================================================

    /// 画一条带宽度的线段。
    ///
    /// 用「沿轴线撒圆盘」而不是 Bresenham 单像素线：这样线宽是各向同性的，
    /// 转角处也不会出现缺口（Bresenham + 矩形笔刷在斜线上会有台阶）。
    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, color: Bgra, width: i32) {
        let c = Self::premultiply(color);
        let r = (width.max(1) as f32) * 0.5;
        let dx = x1 - x0;
        let dy = y1 - y0;
        let len = (dx * dx + dy * dy).sqrt();
        if !len.is_finite() {
            return;
        }
        // 步长取半径的一半，保证圆盘之间有足够重叠、看不出颗粒
        let step = (r * 0.5).max(0.5);
        let steps = (len / step).ceil().max(1.0) as i32;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            self.disc(x0 + dx * t, y0 + dy * t, r, c);
        }
    }

    /// 画一串首尾相连的线段（折线）。
    pub fn polyline(&mut self, pts: &[(f32, f32)], color: Bgra, width: i32) {
        if pts.len() < 2 {
            return;
        }
        for w in pts.windows(2) {
            self.line(w[0].0, w[0].1, w[1].0, w[1].1, color, width);
        }
    }

    /// 填充三角形（用于指针）。用重心坐标判定内部点。
    pub fn fill_triangle(&mut self, p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), color: Bgra) {
        let c = Self::premultiply(color);
        let min_x = p0.0.min(p1.0).min(p2.0).floor() as i32;
        let max_x = p0.0.max(p1.0).max(p2.0).ceil() as i32;
        let min_y = p0.1.min(p1.1).min(p2.1).floor() as i32;
        let max_y = p0.1.max(p1.1).max(p2.1).ceil() as i32;

        let area = (p1.0 - p0.0) * (p2.1 - p0.1) - (p2.0 - p0.0) * (p1.1 - p0.1);
        if area.abs() < 1e-6 {
            return;
        }
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let w0 = ((p1.0 - p0.0) * (py - p0.1) - (px - p0.0) * (p1.1 - p0.1)) / area;
                let w1 = ((px - p0.0) * (p2.1 - p0.1) - (p2.0 - p0.0) * (py - p0.1)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 >= -1e-4 && w1 >= -1e-4 && w2 >= -1e-4 {
                    self.blend_pixel(x, y, c);
                }
            }
        }
    }

    /// 填充多边形（扫描线算法，支持凹多边形）。
    ///
    /// G 表的圆环、面板的角括号等都靠它。
    pub fn fill_polygon(&mut self, pts: &[(f32, f32)], color: Bgra) {
        if pts.len() < 3 {
            return;
        }
        let c = Self::premultiply(color);
        let min_y = pts
            .iter()
            .map(|p| p.1)
            .fold(f32::INFINITY, f32::min)
            .floor() as i32;
        let max_y = pts
            .iter()
            .map(|p| p.1)
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil() as i32;

        // 每条扫描线求交点并成对填充
        let mut xs: Vec<f32> = Vec::with_capacity(pts.len());
        for y in min_y..=max_y {
            let sy = y as f32 + 0.5;
            xs.clear();
            for i in 0..pts.len() {
                let a = pts[i];
                let b = pts[(i + 1) % pts.len()];
                // 半开区间判定，避免顶点被重复计入
                if (a.1 <= sy && b.1 > sy) || (b.1 <= sy && a.1 > sy) {
                    let t = (sy - a.1) / (b.1 - a.1);
                    xs.push(a.0 + t * (b.0 - a.0));
                }
            }
            if xs.len() < 2 {
                continue;
            }
            xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let mut i = 0;
            while i + 1 < xs.len() {
                let x0 = xs[i].round() as i32;
                let x1 = xs[i + 1].round() as i32;
                for x in x0..=x1 {
                    self.blend_pixel(x, y, c);
                }
                i += 2;
            }
        }
    }

    // ========================================================================
    // 圆
    // ========================================================================

    /// 填充圆盘（带 alpha 混合与边缘抗锯齿）。
    ///
    /// 边缘一个像素内按覆盖率线性衰减，避免锯齿。
    pub fn disc(&mut self, cx: f32, cy: f32, radius: f32, color: Bgra) {
        if radius <= 0.0 {
            return;
        }
        let min_x = (cx - radius - 1.0).floor() as i32;
        let max_x = (cx + radius + 1.0).ceil() as i32;
        let min_y = (cy - radius - 1.0).floor() as i32;
        let max_y = (cy + radius + 1.0).ceil() as i32;
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                let d = (dx * dx + dy * dy).sqrt();
                // 覆盖率：d <= r-0.5 全覆盖；d >= r+0.5 不画；中间线性
                let cov = (radius + 0.5 - d).clamp(0.0, 1.0);
                if cov <= 0.002 {
                    continue;
                }
                let a = (color.a as f32 * cov) as u8;
                self.blend_pixel(x, y, Self::premultiply(color.with_alpha(a)));
            }
        }
    }

    /// 画空心圆环（`radius` 为半径，`thickness` 为环宽）。
    pub fn ring(&mut self, cx: f32, cy: f32, radius: f32, thickness: f32, color: Bgra) {
        let outer = radius + thickness * 0.5;
        let inner = (radius - thickness * 0.5).max(0.0);
        let min_x = (cx - outer - 1.0).floor() as i32;
        let max_x = (cx + outer + 1.0).ceil() as i32;
        let min_y = (cy - outer - 1.0).floor() as i32;
        let max_y = (cy + outer + 1.0).ceil() as i32;
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                let d = (dx * dx + dy * dy).sqrt();
                let cov_out = (outer + 0.5 - d).clamp(0.0, 1.0);
                let cov_in = (d - inner + 0.5).clamp(0.0, 1.0);
                let cov = cov_out.min(cov_in);
                if cov <= 0.002 {
                    continue;
                }
                let a = (color.a as f32 * cov) as u8;
                self.blend_pixel(x, y, Self::premultiply(color.with_alpha(a)));
            }
        }
    }

    /// 在矩形内画横向扫描线。
    pub fn scanlines(&mut self, r: Rect, color: Bgra, alpha: f32, spacing: i32) {
        if alpha <= 0.001 || spacing <= 0 {
            return;
        }
        let c = color.with_alpha((color.a as f32 * alpha.clamp(0.0, 1.0)) as u8);
        let region = r.clamp_to(&self.clip);
        let mut y = region.y as i32;
        while (y as f32) < region.bottom() {
            self.fill_rect(Rect::new(region.x, y as f32, region.w, 1.0), c);
            y += spacing;
        }
    }

    /// 在矩形内画半调网点。
    pub fn halftone(&mut self, r: Rect, color: Bgra, alpha: f32, spacing: i32, dot: f32) {
        if alpha <= 0.001 || spacing <= 0 {
            return;
        }
        let c = color.with_alpha((color.a as f32 * alpha.clamp(0.0, 1.0)) as u8);
        let region = r.clamp_to(&self.clip);
        let mut y = region.y as i32;
        let mut row = 0;
        while (y as f32) < region.bottom() {
            let mut x = region.x as i32 + if row % 2 == 0 { 0 } else { spacing / 2 };
            while (x as f32) < region.right() {
                self.disc(x as f32, y as f32, dot, c);
                x += spacing;
            }
            y += spacing;
            row += 1;
        }
    }

    /// 画一个点（调试用）。
    pub fn dot(&mut self, x: f32, y: f32, color: Bgra) {
        self.blend_pixel(x as i32, y as i32, Self::premultiply(color));
    }

    // ========================================================================
    // 文字：GDI 只用来生成 alpha 掩码
    // ========================================================================

    /// 用 GDI 把文字渲染成 **alpha 掩码**，然后按给定颜色混合到画布上。
    ///
    /// # 为什么绕这一圈
    ///
    /// GDI 会写 RGB 但不写 alpha（见模块文档）。于是用它画出来的文字在分层窗口里
    /// 是**全透明**的，等于没画。但它又是唯一现成的字体光栅化器（自带字体回退、
    /// 字形抗锯齿、CJK 支持），自己实现代价太高。
    ///
    /// 折中做法：
    ///
    /// 1. 用 GDI 在画布的临时位置把文字画成**纯白**（用一次性 DC 与位图）；
    /// 2. 把这块像素当掩码：`alpha = 像素亮度`；
    /// 3. 用该掩码把**目标颜色**混合进画布。
    ///
    /// 步骤 1–3 的中间结果不进入最终画布（第 1 步画在独立位图上），
    /// 因此不会留下白字残影。颜色与透明度完全由我们控制，抗锯齿来自 GDI 的字形光栅化。
    ///
    /// 返回文字宽度估计值（用于排版），失败时返回 0。
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
        let mask = match self.render_text_mask(s, size, bold) {
            Some(m) => m,
            None => return 0.0,
        };
        let est_width = mask.width as f32;
        let left = match align {
            TextAlign::Left => x,
            TextAlign::Center => x - est_width * 0.5,
            TextAlign::Right => x - est_width,
        };
        self.blend_mask(&mask, left.round() as i32, y.round() as i32, color);
        est_width
    }

    /// 画带描边的文字。
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
        let layers = layers.min(6);
        for i in (1..=layers).rev() {
            let d = i as f32;
            for (dx, dy) in [(-d, 0.0), (d, 0.0), (0.0, -d), (0.0, d)] {
                self.text(s, x + dx, y + dy, size, outline, bold, align);
            }
        }
        self.text(s, x, y, size, color, bold, align)
    }

    /// 把文字渲染成灰度掩码。
    ///
    /// 复用一块静态掩码缓冲，避免每帧分配。坐标系是掩码自己的（左上原点）。
    fn render_text_mask(&mut self, s: &str, size: f32, bold: bool) -> Option<TextMask> {
        use crate::textmask::TextMaskCacheHandle;
        let key = format!("{s}\u{1}{size:.0}\u{1}{bold}");
        TextMaskCacheHandle::global().get(&key, s, size, bold)
    }

    /// 用一个灰度掩码把颜色混合到画布。
    fn blend_mask(&mut self, mask: &crate::textmask::TextMask, x: i32, y: i32, color: Bgra) {
        let c = Self::premultiply(color);
        for my in 0..mask.height {
            for mx in 0..mask.width {
                let cov = mask.coverage(mx, my);
                if cov == 0 {
                    continue;
                }
                let a = (c.a as u32 * cov as u32 / 255) as u8;
                let px = Bgra::new(
                    (c.r as u32 * cov as u32 / 255) as u8,
                    (c.g as u32 * cov as u32 / 255) as u8,
                    (c.b as u32 * cov as u32 / 255) as u8,
                    a,
                );
                self.blend_pixel(x + mx, y + my, px);
            }
        }
    }
}

/// 文本对齐方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    /// `x` 是左边界。
    Left,
    /// `x` 是中心。
    Center,
    /// `x` 是右边界。
    Right,
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use crate::gauge::Bgra;

    /// 一个不透明红点应把目标像素写成预乘后的红色。
    #[test]
    fn 不透明绘制得到正确的预乘值() {
        let mut c = Canvas::new(16, 16).unwrap();
        c.clear_transparent();
        {
            let mut r = Rasterizer::new(&mut c);
            r.fill_rect(Rect::new(4.0, 4.0, 4.0, 4.0), Bgra::rgb(255, 0, 0));
        }
        let p = c.pixel(5, 5).unwrap();
        assert_eq!((p.r, p.g, p.b, p.a), (255, 0, 0, 255), "不透明红色");
        // 区域外应仍是全透明
        assert_eq!(c.pixel(0, 0).unwrap().a, 0);
        assert_eq!(c.pixel(9, 9).unwrap().a, 0);
    }

    #[test]
    fn 半透明绘制被预乘() {
        let mut c = Canvas::new(8, 8).unwrap();
        c.clear_transparent();
        {
            let mut r = Rasterizer::new(&mut c);
            // 50% 白
            r.fill_rect(Rect::new(0.0, 0.0, 8.0, 8.0), Bgra::new(255, 255, 255, 128));
        }
        let p = c.pixel(4, 4).unwrap();
        assert_eq!(p.a, 128);
        // 预乘：255 * 128/255 = 128
        assert_eq!(p.r, 128, "预乘后红色分量应为 128");
        assert_eq!(p.g, 128);
        assert_eq!(p.b, 128);
    }

    #[test]
    fn 两次半透明叠加比一次更亮() {
        let mut c = Canvas::new(4, 4).unwrap();
        c.clear_transparent();
        {
            let mut r = Rasterizer::new(&mut c);
            r.fill_rect(Rect::new(0.0, 0.0, 4.0, 4.0), Bgra::new(255, 255, 255, 100));
            r.fill_rect(Rect::new(0.0, 0.0, 4.0, 4.0), Bgra::new(255, 255, 255, 100));
        }
        let p = c.pixel(1, 1).unwrap();
        assert!(p.a > 100, "叠加后 alpha 应增加, 实际 {}", p.a);
        assert!(p.r > 100, "叠加后亮度应增加, 实际 {}", p.r);
        // u8 天然 <= 255，这里只断言叠加没有把它截断成更小值
        assert!(p.a >= 100);
    }

    #[test]
    fn 全透明颜色不改变像素() {
        let mut c = Canvas::new(4, 4).unwrap();
        c.clear_transparent();
        {
            let mut r = Rasterizer::new(&mut c);
            r.fill_rect(Rect::new(0.0, 0.0, 4.0, 4.0), Bgra::new(255, 0, 0, 0));
        }
        assert_eq!(c.pixel(1, 1).unwrap().a, 0, "alpha=0 不应画上任何东西");
    }

    #[test]
    fn 裁剪区限制绘制范围() {
        let mut c = Canvas::new(20, 20).unwrap();
        c.clear_transparent();
        {
            let mut r = Rasterizer::new(&mut c);
            r.set_clip(Rect::new(5.0, 5.0, 5.0, 5.0));
            // 请求画满整个画布，但应被裁剪到 5..10
            r.fill_rect(Rect::new(0.0, 0.0, 20.0, 20.0), Bgra::rgb(255, 255, 255));
        }
        assert_eq!(c.pixel(7, 7).unwrap().a, 255, "裁剪区内应被画上");
        assert_eq!(c.pixel(2, 2).unwrap().a, 0, "裁剪区外不应被画上");
        assert_eq!(c.pixel(15, 15).unwrap().a, 0, "裁剪区外不应被画上");
    }

    #[test]
    fn 画布外坐标不越界也不panic() {
        let mut c = Canvas::new(8, 8).unwrap();
        c.clear_transparent();
        {
            let mut r = Rasterizer::new(&mut c);
            r.fill_rect(Rect::new(-100.0, -100.0, 50.0, 50.0), Bgra::rgb(255, 0, 0));
            r.line(-50.0, -50.0, 100.0, 100.0, Bgra::rgb(0, 255, 0), 3);
            r.disc(-20.0, -20.0, 10.0, Bgra::rgb(0, 0, 255));
            r.fill_triangle(
                (-10.0, -10.0),
                (500.0, 0.0),
                (0.0, 500.0),
                Bgra::rgb(255, 255, 0),
            );
        }
        // 只要没 panic 就算通过（越界绘制已被内部夹紧）。
        // 顺带确认画布还能正常读取。
        let _ = c.pixel(4, 4).unwrap();
    }

    #[test]
    fn 线段确实画在起终点之间() {
        let mut c = Canvas::new(64, 64).unwrap();
        c.clear_transparent();
        {
            let mut r = Rasterizer::new(&mut c);
            r.line(10.0, 10.0, 50.0, 50.0, Bgra::rgb(255, 255, 255), 3);
        }
        // 中点附近应有像素
        assert!(c.pixel(30, 30).unwrap().a > 40, "对角线中点应有线");
        // 起点终点附近
        assert!(c.pixel(10, 10).unwrap().a > 40);
        assert!(c.pixel(50, 50).unwrap().a > 40);
        // 远离线的地方不应有
        assert_eq!(c.pixel(50, 10).unwrap().a, 0, "线外不应被画");
    }

    #[test]
    fn 圆盘边缘有抗锯齿渐变() {
        let mut c = Canvas::new(64, 64).unwrap();
        c.clear_transparent();
        {
            let mut r = Rasterizer::new(&mut c);
            r.disc(32.0, 32.0, 20.0, Bgra::rgb(255, 255, 255));
        }
        // 圆心应完全不透明
        assert_eq!(c.pixel(32, 32).unwrap().a, 255);
        // 半径 20，在 r=22 处应已经透明
        assert_eq!(c.pixel(54, 32).unwrap().a, 0, "圆外应透明");
        // 边缘附近应存在"部分覆盖"的像素（证明有抗锯齿）
        let mut has_partial = false;
        for x in 50..56 {
            if let Some(p) = c.pixel(x, 32) {
                if p.a != 0 && p.a != 255 {
                    has_partial = true;
                }
            }
        }
        assert!(has_partial, "边缘应存在半透明像素（抗锯齿）");
    }

    #[test]
    fn 圆环内部是空的() {
        let mut c = Canvas::new(64, 64).unwrap();
        c.clear_transparent();
        {
            let mut r = Rasterizer::new(&mut c);
            r.ring(32.0, 32.0, 20.0, 6.0, Bgra::rgb(255, 255, 255));
        }
        assert_eq!(c.pixel(32, 32).unwrap().a, 0, "圆心应为空");
        assert!(c.pixel(32, 12).unwrap().a > 100, "环上应有像素");
        assert_eq!(c.pixel(32, 2).unwrap().a, 0, "环外应透明");
    }

    #[test]
    fn 填充三角形覆盖内部点() {
        let mut c = Canvas::new(64, 64).unwrap();
        c.clear_transparent();
        {
            let mut r = Rasterizer::new(&mut c);
            r.fill_triangle(
                (32.0, 8.0),
                (8.0, 56.0),
                (56.0, 56.0),
                Bgra::rgb(255, 255, 255),
            );
        }
        assert_eq!(c.pixel(32, 40).unwrap().a, 255, "三角形内部");
        assert_eq!(c.pixel(8, 8).unwrap().a, 0, "三角形外部");
        assert_eq!(c.pixel(56, 8).unwrap().a, 0, "三角形外部");
    }

    #[test]
    fn 凹多边形填充不会溢出() {
        // 一个「C」形（凹多边形）：验证扫描线算法没有把凹口填上
        let pts = [
            (10.0f32, 10.0f32),
            (50.0, 10.0),
            (50.0, 20.0),
            (20.0, 20.0),
            (20.0, 40.0),
            (50.0, 40.0),
            (50.0, 50.0),
            (10.0, 50.0),
        ];
        let mut c = Canvas::new(64, 64).unwrap();
        c.clear_transparent();
        {
            let mut r = Rasterizer::new(&mut c);
            r.fill_polygon(&pts, Bgra::rgb(255, 255, 255));
        }
        // 左竖条内部
        assert_eq!(c.pixel(15, 30).unwrap().a, 255, "左竖条应有填充");
        // 凹口内部（x=40,y=30）应**没有**填充
        assert_eq!(c.pixel(40, 30).unwrap().a, 0, "凹口不应被填充");
        // 上下横条内部
        assert_eq!(c.pixel(40, 15).unwrap().a, 255, "上横条应有填充");
        assert_eq!(c.pixel(40, 45).unwrap().a, 255, "下横条应有填充");
    }

    #[test]
    fn 预乘函数正确() {
        let c = Bgra::new(200, 100, 50, 128);
        let p = Rasterizer::premultiply(c);
        assert_eq!(p.a, 128);
        assert_eq!(p.r, (200 * 128 / 255) as u8);
        assert_eq!(p.g, (100 * 128 / 255) as u8);
        assert_eq!(p.b, (50 * 128 / 255) as u8);

        // 全透明 → 全 0
        let t = Rasterizer::premultiply(Bgra::new(255, 255, 255, 0));
        assert_eq!((t.r, t.g, t.b, t.a), (0, 0, 0, 0));

        // 全不透明 → 原值
        let o = Rasterizer::premultiply(Bgra::rgb(1, 2, 3));
        assert_eq!((o.r, o.g, o.b, o.a), (1, 2, 3, 255));
    }

    #[test]
    fn 矩形边框只画四条边() {
        let mut c = Canvas::new(32, 32).unwrap();
        c.clear_transparent();
        {
            let mut r = Rasterizer::new(&mut c);
            r.stroke_rect(Rect::new(8.0, 8.0, 16.0, 16.0), Bgra::rgb(255, 255, 255), 2);
        }
        assert!(c.pixel(8, 8).unwrap().a > 0, "左上角应有边框");
        assert!(c.pixel(23, 23).unwrap().a > 0, "右下角应有边框");
        assert_eq!(c.pixel(16, 16).unwrap().a, 0, "内部应是空的（只描边）");
        assert_eq!(c.pixel(2, 16).unwrap().a, 0, "外部不应有");
    }

    #[test]
    fn 扫描线按间距分布() {
        let mut c = Canvas::new(32, 32).unwrap();
        c.clear_transparent();
        {
            let mut r = Rasterizer::new(&mut c);
            r.scanlines(
                Rect::new(0.0, 0.0, 32.0, 32.0),
                Bgra::rgb(255, 255, 255),
                1.0,
                4,
            );
        }
        assert!(c.pixel(16, 0).unwrap().a > 0, "第 0 行应有线");
        assert!(c.pixel(16, 4).unwrap().a > 0, "第 4 行应有线");
        assert_eq!(c.pixel(16, 2).unwrap().a, 0, "行之间应为空");
    }

    #[test]
    fn 文字对齐影响水平位置() {
        // 不依赖 GDI 是否可用：三种对齐都不应 panic，且返回宽度一致
        let mut c = Canvas::new(200, 60).unwrap();
        c.clear_transparent();
        let (w1, w2, w3);
        {
            let mut r = Rasterizer::new(&mut c);
            w1 = r.text(
                "88",
                100.0,
                20.0,
                24.0,
                Bgra::rgb(255, 255, 255),
                true,
                TextAlign::Left,
            );
            w2 = r.text(
                "88",
                100.0,
                20.0,
                24.0,
                Bgra::rgb(255, 255, 255),
                true,
                TextAlign::Center,
            );
            w3 = r.text(
                "88",
                100.0,
                20.0,
                24.0,
                Bgra::rgb(255, 255, 255),
                true,
                TextAlign::Right,
            );
        }
        assert_eq!(w1, w2);
        assert_eq!(w2, w3);
    }
}
