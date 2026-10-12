//! 文本 alpha 掩码：用 GDI 做字形光栅化，但只为拿到「覆盖率」。
//!
//! # 为什么需要这一层
//!
//! 分层窗口按**每像素 alpha** 合成，而 GDI **不写 alpha 通道**（实测见
//! [`crate::raster`] 的模块文档）。所以直接用 GDI 画文字，结果在屏幕上是
//! 全透明的 —— 等于没画。
//!
//! 但 GDI 又是唯一现成的字形光栅化器（自带字体回退、抗锯齿、CJK 支持），
//! 自己实现一套字体引擎的代价远超收益。于是折中：
//!
//! 1. 用 GDI 在一张**独立的小位图**上把文字画成**纯白**；
//! 2. 读回像素，把亮度当作**覆盖率**（0..255）；
//! 3. 交给 [`crate::raster`] 用这个覆盖率把任意颜色混合到画布上。
//!
//! 这样既拿到了 GDI 的字形质量，又完全掌控了颜色与透明度，
//! 而且第 1 步的中间像素**不会进入最终画布**，没有白字残影。
//!
//! # 缓存
//!
//! 文字内容有限（挡位、数字、几个固定标签），字形光栅化又相对昂贵
//! （要建 DC、建位图、选字体、画字、读回）。因此按「文本 + 字号 + 粗细」
//! 做缓存，容量上限 512 条，超出后整体清空（简单且命中率足够）。

use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr;

use crate::win32 as w;

/// 一块文字掩码。
#[derive(Debug, Clone)]
pub struct TextMask {
    /// 宽（像素）。
    pub width: i32,
    /// 高（像素）。
    pub height: i32,
    /// 每像素的覆盖率，长度 = width * height。
    coverage: Vec<u8>,
}

impl TextMask {
    /// 取某点的覆盖率（0..255）。越界返回 0。
    #[inline]
    pub fn coverage(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return 0;
        }
        let i = (y as usize) * (self.width as usize) + (x as usize);
        self.coverage.get(i).copied().unwrap_or(0)
    }

    /// 掩码中是否有任何非零覆盖率（用于判断渲染是否真的成功）。
    pub fn is_blank(&self) -> bool {
        self.coverage.iter().all(|&c| c == 0)
    }
}

/// 掩码缓存上限。超过就整体清空。
const CACHE_LIMIT: usize = 512;

/// 文字掩码缓存。
struct TextMaskCache {
    map: HashMap<String, TextMask>,
    /// 是否已确认 GDI 文本渲染不可用（例如字体缺失）。
    broken: bool,
}

thread_local! {
    /// 每个线程一份缓存。
    ///
    /// 用 `thread_local` 而不是全局 `Mutex`：渲染只在一个线程里发生，
    /// 这样完全没有锁开销；而且 GDI 对象本身就有线程亲和性，跨线程复用
    /// 反而危险。
    static CACHE: RefCell<TextMaskCache> = RefCell::new(TextMaskCache {
        map: HashMap::new(),
        broken: false,
    });
}

impl TextMaskCache {
    /// 取得（必要时渲染）一个文字掩码。
    fn get(&mut self, key: &str, text: &str, size: f32, bold: bool) -> Option<TextMask> {
        if self.broken {
            return None;
        }
        if let Some(m) = self.map.get(key) {
            return Some(m.clone());
        }
        if self.map.len() >= CACHE_LIMIT {
            self.map.clear();
        }
        match render_text_mask(text, size, bold) {
            Some(m) => {
                self.map.insert(key.to_string(), m.clone());
                Some(m)
            }
            None => {
                // 只标记一次，避免每帧重复尝试并重复失败
                self.broken = true;
                None
            }
        }
    }
}

/// 供 [`crate::raster::Rasterizer`] 使用的入口。
pub struct TextMaskCacheHandle;

impl TextMaskCacheHandle {
    /// 全局缓存的便捷写法。
    pub fn global() -> &'static TextMaskCacheHandle {
        &TextMaskCacheHandle
    }

    /// 取掩码。
    pub fn get(&self, key: &str, text: &str, size: f32, bold: bool) -> Option<TextMask> {
        CACHE.with(|c| c.borrow_mut().get(key, text, size, bold))
    }
}

/// 实际用 GDI 渲染一次文字掩码。
///
/// 流程：建内存 DC → 建 32 位 DIB → 选入白色字体 → 画字 →
/// 读回像素取最亮通道作为覆盖率 → 释放全部 GDI 对象。
fn render_text_mask(text: &str, size: f32, bold: bool) -> Option<TextMask> {
    if text.is_empty() {
        return None;
    }
    let px = size.round().max(4.0);
    // 尺寸估算：等宽上限 + 余量。汉字比拉丁字母宽，按 1.0 * size 估更稳。
    let width = ((text.chars().count() as f32) * px * 1.05 + px).ceil() as i32;
    let height = (px * 2.0).ceil() as i32;
    if width <= 0 || height <= 0 || width > 4096 || height > 4096 {
        return None;
    }

    // SAFETY: 以下整段都是对 Win32 的调用；每个句柄都在同一次调用内成对创建与释放，
    // 且指针参数都指向本函数栈上的有效数据。
    unsafe {
        let screen_dc = w::GetDC(ptr::null_mut());
        if screen_dc.is_null() {
            return None;
        }
        let mem_dc = w::CreateCompatibleDC(screen_dc);
        w::ReleaseDC(ptr::null_mut(), screen_dc);
        if mem_dc.is_null() {
            return None;
        }

        let bi = w::BITMAPINFO::top_down_32bpp(width, height);
        let mut bits: *mut std::ffi::c_void = ptr::null_mut();
        let bitmap = w::CreateDIBSection(mem_dc, &bi, 0, &mut bits, ptr::null_mut(), 0);
        if bitmap.is_null() || bits.is_null() {
            w::DeleteDC(mem_dc);
            return None;
        }
        let old_bitmap = w::SelectObject(mem_dc, bitmap);

        // 用纯白画字、并在读取时取最大通道 —— 这样无论 GDI 是否做子像素渲染，
        // 覆盖率都单调可用。背景初始就是全 0（透明黑），不需要额外擦除。
        let face = w::to_wide("Bahnschrift");
        let font = w::CreateFontW(
            -(px as i32),
            0,
            0,
            0,
            if bold { w::fw::BOLD } else { w::fw::NORMAL },
            0,
            0,
            0,
            1, // DEFAULT_CHARSET
            0,
            0,
            5, // CLEARTYPE_QUALITY
            0,
            face.as_ptr(),
        );
        // 若 Bahnschrift 不存在，GDI 会替换字体但仍返回有效句柄；返回 0 才算失败
        if font.is_null() {
            w::SelectObject(mem_dc, old_bitmap);
            w::DeleteObject(bitmap);
            w::DeleteDC(mem_dc);
            return None;
        }
        let old_font = w::SelectObject(mem_dc, font);
        w::SetBkMode(mem_dc, w::TRANSPARENT_BK);
        // 用白色前景；SetBkColor 为黑，配合 TRANSPARENT 模式不影响未覆盖像素
        w::SetTextColor(mem_dc, w::rgb(255, 255, 255));
        w::SetBkColor(mem_dc, w::rgb(0, 0, 0));

        let wide: Vec<u16> = text.encode_utf16().collect();
        w::TextOutW(
            mem_dc,
            (px * 0.5) as i32,
            (px * 0.25) as i32,
            wide.as_ptr(),
            wide.len() as i32,
        );
        // 必须刷新，确保 GDI 的绘制已落到 DIB 内存
        w::GdiFlush();

        // 读回像素，取覆盖率的近似：max(R,G,B)。
        //
        // 说明：ClearType 子像素渲染会产生带彩边的字形。取最大通道相当于
        // 「按最亮的子像素算覆盖率」，对最终观感影响很小（我们后续会用
        // 目标颜色重新着色），但能保住抗锯齿的平滑度。
        let n = (width as usize) * (height as usize);
        let src = std::slice::from_raw_parts(bits as *const u32, n);
        let mut coverage = vec![0u8; n];
        for (i, v) in src.iter().enumerate() {
            let [b, g, r, _a] = v.to_le_bytes();
            coverage[i] = r.max(g).max(b);
        }

        // 释放（顺序与创建相反）
        w::SelectObject(mem_dc, old_font);
        w::DeleteObject(font);
        w::SelectObject(mem_dc, old_bitmap);
        w::DeleteObject(bitmap);
        w::DeleteDC(mem_dc);

        let mask = TextMask {
            width,
            height,
            coverage,
        };
        if mask.is_blank() {
            return None;
        }
        Some(mask)
    }
}

/// 估算一段文本的像素宽度（用于排版，不需要真的渲染）。
///
/// 与 [`crate::raster::Rasterizer::text`] 返回的宽度保持一致，
/// 这样调用方可以在渲染前先算布局。
pub fn estimate_text_width(text: &str, size: f32) -> f32 {
    let px = size.round().max(4.0);
    (text.chars().count() as f32) * px * 1.05 + px
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    #[test]
    fn 宽度估算随字符数与字号增长() {
        let a = estimate_text_width("1", 20.0);
        let b = estimate_text_width("11", 20.0);
        let c = estimate_text_width("1", 40.0);
        assert!(b > a, "字符多应更宽");
        assert!(c > a, "字号大应更宽");
        assert!(a > 0.0);
    }

    #[test]
    fn 空文本估算为零宽但仍有基础余量() {
        // 空串应返回余量（保证调用方不会得到 0 而除零）
        assert!(estimate_text_width("", 20.0) > 0.0);
    }

    #[test]
    fn 覆盖率越界读取返回零() {
        let m = TextMask {
            width: 2,
            height: 2,
            coverage: vec![10, 20, 30, 40],
        };
        assert_eq!(m.coverage(0, 0), 10);
        assert_eq!(m.coverage(1, 1), 40);
        assert_eq!(m.coverage(-1, 0), 0);
        assert_eq!(m.coverage(0, -1), 0);
        assert_eq!(m.coverage(2, 0), 0);
        assert_eq!(m.coverage(0, 2), 0);
        assert_eq!(m.coverage(999, 999), 0);
    }

    #[test]
    fn 空白掩码可被识别() {
        let blank = TextMask {
            width: 2,
            height: 2,
            coverage: vec![0, 0, 0, 0],
        };
        assert!(blank.is_blank());
        let non = TextMask {
            width: 2,
            height: 2,
            coverage: vec![0, 0, 1, 0],
        };
        assert!(!non.is_blank());
    }

    #[test]
    fn 空字符串不产生掩码() {
        assert!(render_text_mask("", 24.0, false).is_none());
    }

    #[test]
    fn 渲染拉丁与中文都能得到非空掩码() {
        // 这条测试同时验证：GDI 文本通路可用、字体回退可用、CJK 可用。
        // 若某个环境下字体服务异常，这里会失败并给出明确信号，
        // 而不是让 HUD 静默地不显示任何文字。
        for s in ["88", "R", "120 km/h", "漂移", "SPIN SAVE!"] {
            let m = render_text_mask(s, 24.0, true);
            assert!(m.is_some(), "文本 `{s}` 应能渲染出掩码（检查字体服务）");
            let m = m.unwrap();
            assert!(m.width > 0 && m.height > 0);
            assert!(!m.is_blank(), "文本 `{s}` 的掩码不应是空白");
        }
    }

    #[test]
    fn 掩码带抗锯齿的中间灰度() {
        let m = match render_text_mask("O", 48.0, false) {
            Some(m) => m,
            None => return, // 环境不支持则跳过（上面已有测试专门检查可用性）
        };
        // 字形边缘应存在既非 0 也非 255 的覆盖率
        let has_partial = m.coverage.iter().any(|&c| c > 0 && c < 255);
        assert!(has_partial, "字形边缘应有抗锯齿的中间值");
    }

    #[test]
    fn 缓存命中返回相同内容() {
        let key = "\u{1}test-cache-key";
        let a = CACHE.with(|c| c.borrow_mut().get(key, "42", 20.0, true));
        let b = CACHE.with(|c| c.borrow_mut().get(key, "42", 20.0, true));
        match (a, b) {
            (Some(x), Some(y)) => {
                assert_eq!(x.width, y.width);
                assert_eq!(x.height, y.height);
                assert_eq!(x.coverage, y.coverage, "缓存应返回同样内容");
            }
            (None, None) => {}
            _ => panic!("两次取掩码的结果应一致"),
        }
    }

    #[test]
    fn 字号变化产生不同的键() {
        let m1 = CACHE.with(|c| c.borrow_mut().get("k1", "7", 20.0, false));
        let m2 = CACHE.with(|c| c.borrow_mut().get("k2", "7", 40.0, false));
        if let (Some(a), Some(b)) = (m1, m2) {
            assert!(b.width > a.width, "更大字号应产生更宽的掩码");
        }
    }
}
