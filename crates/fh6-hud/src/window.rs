//! HUD 叠加窗口：透明、置顶、点击穿透。
//!
//! # 窗口样式的选择理由
//!
//! | 样式 | 作用 | 为什么必须 |
//! |---|---|---|
//! | `WS_EX_LAYERED` | 支持每像素 alpha | 没有它就无法做到真正的半透明，只能抠色，边缘会有锯齿 |
//! | `WS_EX_TRANSPARENT` | **鼠标事件穿透** | 参考实现没做这个，而它把窗口设为整屏，于是鼠标被叠加层吃掉、挡到游戏操作 |
//! | `WS_EX_TOPMOST` | 总在最前 | 否则全屏游戏会盖住 HUD |
//! | `WS_EX_TOOLWINDOW` | 不在任务栏、不参与 Alt+Tab | 避免切窗口时出现一个无用的 HUD 条目 |
//! | `WS_EX_NOACTIVATE` | 不接受焦点 | 否则点击（即使穿透失败）会把焦点从游戏抢走，游戏会最小化 |
//! | `WS_POPUP` | 无标题栏无边框 | 有边框就不是叠加层了 |
//!
//! # 渲染方式
//!
//! 用 `UpdateLayeredWindow` 而不是「`WM_PAINT` + `BitBlt`」，因为：
//!
//! 1. 分层窗口的自绘路径在部分驱动上会闪（需要额外的 `WM_ERASEBKGND` 处理）；
//! 2. `UpdateLayeredWindow` 一次性提交整块带 alpha 的位图，合成由 DWM 负责，
//!    HUD 进程**不需要参与每帧的窗口合成**；
//! 3. 可以直接传入脏区矩形，只更新变化的部分。
//!
//! # safe 边界
//!
//! 本模块是全项目**唯一**调用 Win32 的地方。所有 `unsafe` 都紧贴调用点并
//! 标注 `// SAFETY:` 说明前置条件，便于集中审查。

use std::ffi::c_void;
use std::ptr;

use crate::gauge::{Bgra, Rect};
use crate::win32::{self as w, to_wide};

/// 窗口类的名字（注册到系统）。
const CLASS_NAME: &str = "Fh6HudOverlayWindow";

/// 离屏画布：一块「自上而下、32 位 BGRA」的 DIB 内存。
///
/// 之所以要自上而下（负 `biHeight`）：这样缓冲区里第 0 行就对应屏幕最上方，
/// 写像素时 `buffer[y * width + x]` 的 `y` 与屏幕坐标方向一致，
/// 不需要在每次绘制时做 `height - 1 - y` 的翻转（那是最容易写错的地方之一）。
pub struct Canvas {
    width: i32,
    height: i32,
    pixels: *mut u32,
    bitmap: w::HBITMAP,
    mem_dc: w::HDC,
}

impl Canvas {
    /// 创建指定尺寸的画布。
    ///
    /// 公开是为了让 --selftest 能在**不创建窗口**的情况下渲染一帧，
    /// 从而让绘制代码可以在没有 GPU / 不盖住屏幕的前提下被核对。
    pub fn new(width: i32, height: i32) -> Result<Self, String> {
        if width <= 0 || height <= 0 {
            return Err(format!("画布尺寸非法: {width}x{height}"));
        }

        // SAFETY: CreateCompatibleDC(NULL) 返回与屏幕兼容的内存 DC，失败返回 NULL。
        let screen_dc = unsafe { w::GetDC(ptr::null_mut()) };
        if screen_dc.is_null() {
            return Err("GetDC 失败".to_string());
        }
        // SAFETY: screen_dc 来自 GetDC，此处配对 CreateCompatibleDC。
        let mem_dc = unsafe { w::CreateCompatibleDC(screen_dc) };
        // SAFETY: screen_dc 必须配对 ReleaseDC；mem_dc 的生命周期独立于它。
        unsafe { w::ReleaseDC(ptr::null_mut(), screen_dc) };
        if mem_dc.is_null() {
            return Err("CreateCompatibleDC 失败".to_string());
        }

        let bi = w::BITMAPINFO::top_down_32bpp(width, height);
        let mut bits: *mut c_void = ptr::null_mut();
        // SAFETY: bi 是合法的 BITMAPINFO；bits 是有效的出参指针；
        // hSection 传 NULL、offset 传 0 表示由系统分配内存。
        let bitmap = unsafe {
            w::CreateDIBSection(
                mem_dc,
                &bi,
                0, /* DIB_RGB_COLORS */
                &mut bits,
                ptr::null_mut(),
                0,
            )
        };
        if bitmap.is_null() || bits.is_null() {
            // SAFETY: mem_dc 是本函数创建的，尚未使用，可直接删除。
            unsafe { w::DeleteDC(mem_dc) };
            return Err("CreateDIBSection 失败（可能是显存/内存不足）".to_string());
        }

        // 把位图选进 DC，之后所有 GDI 绘制都作用在这块内存上
        // SAFETY: bitmap 是刚创建的 DIB，mem_dc 是刚创建的内存 DC。
        unsafe { w::SelectObject(mem_dc, bitmap) };

        Ok(Self {
            width,
            height,
            pixels: bits as *mut u32,
            bitmap,
            mem_dc,
        })
    }

    /// 画布宽度。
    pub fn width(&self) -> i32 {
        self.width
    }

    /// 画布高度。
    pub fn height(&self) -> i32 {
        self.height
    }

    /// 尺寸（宽, 高）。
    pub fn size(&self) -> (i32, i32) {
        (self.width, self.height)
    }

    /// 底层的 GDI 内存 DC，供绘制后端直接使用。
    pub fn dc(&self) -> w::HDC {
        self.mem_dc
    }

    /// 把整块画布填成完全透明。
    ///
    /// 直接写内存而不是用 GDI 的 `PatBlt`：整屏 1920×1080 有 200 万像素，
    /// 用 `fill` 是内存带宽量级（约 8 MB），而 GDI 填充会走一遍图元管线。
    pub fn clear_transparent(&mut self) {
        if self.pixels.is_null() {
            return;
        }
        let n = (self.width as usize) * (self.height as usize);
        // SAFETY: pixels 指向 CreateDIBSection 分配的 n 个 u32，写 n 个元素在界内。
        unsafe {
            let s = std::slice::from_raw_parts_mut(self.pixels, n);
            s.fill(0);
        }
    }

    /// 把指定矩形填成给定颜色（含 alpha）。
    ///
    /// 矩形会被裁剪到画布范围内。
    pub fn fill_rect(&mut self, r: Rect, color: Bgra) {
        if self.pixels.is_null() {
            return;
        }
        let canvas = Rect::new(0.0, 0.0, self.width as f32, self.height as f32);
        let clipped = r.clamp_to(&canvas);
        if clipped.w < 1.0 || clipped.h < 1.0 {
            return;
        }
        let x0 = clipped.x as i32;
        let y0 = clipped.y as i32;
        let x1 = clipped.right().ceil() as i32;
        let y1 = clipped.bottom().ceil() as i32;
        let value = u32::from_le_bytes([color.b, color.g, color.r, color.a]);

        // SAFETY: 逐行写入，索引已按画布尺寸裁剪，保证在界内。
        unsafe {
            for y in y0..y1 {
                let row = self.pixels.add(y as usize * self.width as usize);
                for x in x0..x1 {
                    *row.add(x as usize) = value;
                }
            }
        }
    }

    /// 写入一个像素（预乘 BGRA）。
    ///
    /// 供软件光栅化器使用：它需要「读改写」（source-over 混合），
    /// 所以既要有读也要有写。
    #[inline]
    pub fn put_pixel(&mut self, x: i32, y: i32, color: Bgra) {
        if self.pixels.is_null() || x < 0 || y < 0 || x >= self.width || y >= self.height {
            return;
        }
        let v = u32::from_le_bytes([color.b, color.g, color.r, color.a]);
        // SAFETY: 索引已做边界检查。
        unsafe {
            *self
                .pixels
                .add(y as usize * self.width as usize + x as usize) = v;
        }
    }
    /// 读取一个像素（调试与测试用）。
    ///
    /// 越界返回 `None`。
    pub fn pixel(&self, x: i32, y: i32) -> Option<Bgra> {
        if self.pixels.is_null() || x < 0 || y < 0 || x >= self.width || y >= self.height {
            return None;
        }
        // SAFETY: 索引已做边界检查。
        let v = unsafe {
            *self
                .pixels
                .add(y as usize * self.width as usize + x as usize)
        };
        let [b, g, r, a] = v.to_le_bytes();
        Some(Bgra::new(r, g, b, a))
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        // SAFETY: 这两个句柄都是本结构体独占持有的，且在此之后不再使用。
        unsafe {
            if !self.bitmap.is_null() {
                w::DeleteObject(self.bitmap);
            }
            if !self.mem_dc.is_null() {
                w::DeleteDC(self.mem_dc);
            }
        }
    }
}

/// HUD 的叠加窗口。
pub struct LayeredWindow {
    hwnd: w::HWND,
    canvas: Canvas,
    /// 窗口在虚拟屏幕上的左上角坐标。
    origin: (i32, i32),
    /// 请求退出（收到 `WM_CLOSE` / Esc 或外部调用 [`LayeredWindow::request_quit`]）。
    quit: bool,
    /// 上次上传到窗口的脏区（用于统计与去重）。
    last_upload: Option<Rect>,
    /// 累计上传次数与字节数，用于统计。
    uploads: u64,
    uploaded_pixels: u64,
    /// 窗口消息回调的上下文指针。
    ///
    /// 我们只有一个窗口，因此用 `Box<Context>` 的裸指针挂在窗口用户数据上；
    /// 生命周期由 `LayeredWindow` 保证（`Drop` 时清空并释放）。
    context: *mut Context,
}

/// 窗口过程需要用到的、但存在 `LayeredWindow` 里的状态。
///
/// 之所以要单独放一个 `Context`：`wnd_proc` 是系统回调，拿不到 `self`，
/// 只能通过 `GWLP_USERDATA` 取一个裸指针。这里只放**回调真正需要**的字段，
/// 避免把整个 `LayeredWindow`（含 Canvas）暴露给回调。
struct Context {
    quit: bool,
    /// 待处理的按键队列（简单环形缓冲，避免在回调里分配）。
    keys: Vec<KeyEvent>,
    /// 鼠标位置（客户区坐标）。
    mouse: (i32, i32),
    mouse_left_down: bool,
    mouse_right_down: bool,
    wheel_delta: i32,
    /// 是否请求重绘（用户操作导致）。
    needs_render: bool,
}

/// 一次按键事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    /// 虚拟键码。
    pub vk: i32,
    /// 是否按下了 Ctrl。
    pub ctrl: bool,
    /// 是否按下了 Shift。
    pub shift: bool,
}

/// 窗口创建参数。
#[derive(Debug, Clone, Copy)]
pub struct WindowOptions {
    /// 左上角 X（虚拟屏幕坐标）。
    pub x: i32,
    /// 左上角 Y。
    pub y: i32,
    /// 宽度。
    pub width: i32,
    /// 高度。
    pub height: i32,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }
    }
}

impl WindowOptions {
    /// 覆盖整个虚拟屏幕（所有显示器）。
    pub fn full_virtual_screen() -> Self {
        // SAFETY: GetSystemMetrics 无前置条件。
        let (x, y, cx, cy) = unsafe {
            (
                w::GetSystemMetrics(w::SM_XVIRTUALSCREEN),
                w::GetSystemMetrics(w::SM_YVIRTUALSCREEN),
                w::GetSystemMetrics(w::SM_CXVIRTUALSCREEN),
                w::GetSystemMetrics(w::SM_CYVIRTUALSCREEN),
            )
        };
        // 虚拟屏幕尺寸在极端情况下可能返回 0（例如远程会话尚未建立），给出兜底
        if cx <= 0 || cy <= 0 {
            // SAFETY: 同上。
            let (sx, sy) = unsafe {
                (
                    w::GetSystemMetrics(w::SM_CXSCREEN),
                    w::GetSystemMetrics(w::SM_CYSCREEN),
                )
            };
            return Self {
                x: 0,
                y: 0,
                width: sx.max(640),
                height: sy.max(480),
            };
        }
        Self {
            x,
            y,
            width: cx,
            height: cy,
        }
    }
}

impl LayeredWindow {
    /// 创建叠加窗口。
    pub fn create(opts: WindowOptions) -> Result<Self, String> {
        let canvas = Canvas::new(opts.width, opts.height)?;

        let class_name = to_wide(CLASS_NAME);
        let window_title = to_wide("FH6 HUD");

        // SAFETY: GetModuleHandleW(NULL) 取当前模块句柄，无失败风险（总有当前模块）。
        let hinstance = unsafe { w::GetModuleHandleW(ptr::null()) };
        if hinstance.is_null() {
            return Err("GetModuleHandleW 失败".to_string());
        }

        let wc = w::WNDCLASSEXW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance,
            lpszClassName: class_name.as_ptr(),
            // 背景不留画刷：分层窗口全部由 UpdateLayeredWindow 提供像素
            hbrBackground: ptr::null_mut(),
            ..Default::default()
        };
        // SAFETY: wc 的所有指针字段指向本函数内有效的、以 NUL 结尾的宽字符串。
        let atom = unsafe { w::RegisterClassExW(&wc) };
        if atom == 0 {
            return Err("RegisterClassExW 失败".to_string());
        }

        let context = Box::into_raw(Box::new(Context {
            quit: false,
            keys: Vec::with_capacity(32),
            mouse: (0, 0),
            mouse_left_down: false,
            mouse_right_down: false,
            wheel_delta: 0,
            needs_render: true,
        }));

        let ex_style = w::ws_ex::LAYERED
            | w::ws_ex::TRANSPARENT
            | w::ws_ex::TOPMOST
            | w::ws_ex::TOOLWINDOW
            | w::ws_ex::NOACTIVATE;
        let style = w::ws::POPUP;

        // SAFETY: 类名与窗口标题都是有效的 NUL 结尾宽字符串；
        // 其余参数为普通整数与空句柄；lpParam 传入刚泄漏出来的 Context 指针，
        // 由 WM_NCCREATE 阶段的窗口过程存进 GWLP_USERDATA。
        let hwnd = unsafe {
            w::CreateWindowExW(
                ex_style,
                class_name.as_ptr(),
                window_title.as_ptr(),
                style,
                opts.x,
                opts.y,
                opts.width,
                opts.height,
                ptr::null_mut(),
                ptr::null_mut(),
                hinstance,
                context as *mut c_void,
            )
        };
        if hwnd.is_null() {
            // SAFETY: 创建失败，context 从未交给窗口，必须由我们收回。
            unsafe { drop(Box::from_raw(context)) };
            return Err("CreateWindowExW 失败".to_string());
        }

        // 把 Context 挂到窗口用户数据上，供 wnd_proc 取回。
        //
        // 注意时机：CreateWindowExW 期间系统可能已经投递过若干消息
        // （如 WM_NCCREATE / WM_SIZE），那些消息到达时用户数据还是 NULL，
        // wnd_proc 里对空指针做了兜底（不处理），不会崩。
        // 这里补挂之后，后续所有消息都能拿到 Context。
        // SAFETY: hwnd 有效；context 是本函数的 Box::into_raw 结果，
        // 生命周期由 LayeredWindow::drop 负责，此处只存指针不转移所有权。
        unsafe {
            w::SetWindowLongPtrW(hwnd, w::GWLP_USERDATA, context as isize);
        }

        let mut win = Self {
            hwnd,
            canvas,
            origin: (opts.x, opts.y),
            quit: false,
            last_upload: None,
            uploads: 0,
            uploaded_pixels: 0,
            context,
        };

        // 首次上传一帧全透明内容，让窗口有合法的分层表面；
        // 否则在第一次真正的 UpdateLayeredWindow 之前窗口可能是未定义外观。
        win.canvas.clear_transparent();
        win.upload(None)?;

        // SAFETY: hwnd 有效；SWP 标志均为常量。
        unsafe {
            w::SetWindowPos(
                hwnd,
                w::HWND_TOPMOST,
                0,
                0,
                0,
                0,
                w::swp::NOMOVE | w::swp::NOSIZE | w::swp::NOACTIVATE | w::swp::SHOWWINDOW,
            );
        }

        Ok(win)
    }

    /// 窗口句柄。
    pub fn hwnd(&self) -> w::HWND {
        self.hwnd
    }

    /// 画布的可变引用（绘制后端在提交前写这里）。
    pub fn canvas_mut(&mut self) -> &mut Canvas {
        &mut self.canvas
    }

    /// 画布的不可变引用。
    pub fn canvas(&self) -> &Canvas {
        &self.canvas
    }

    /// 已经收到退出请求。
    pub fn should_quit(&self) -> bool {
        self.quit
    }

    /// 外部请求退出（例如收到 Ctrl+C）。
    pub fn request_quit(&mut self) {
        self.quit = true;
        // SAFETY: hwnd 有效。投递一条自定义消息唤醒消息循环。
        unsafe {
            w::PostMessageW(self.hwnd, w::wm::APP_QUIT, 0, 0);
        }
    }

    /// 取走上一次退出请求。
    pub fn take_quit(&mut self) -> bool {
        let q = self.quit;
        self.quit = false;
        q
    }

    /// 处理窗口消息队列（非阻塞）。
    ///
    /// 返回 `true` 表示处理过至少一条消息。
    pub fn pump_messages(&mut self) -> bool {
        let mut msg = w::MSG::default();
        let mut handled_any = false;
        loop {
            // SAFETY: msg 是有效的出参；hwnd 传 NULL 表示取本线程所有窗口的消息；
            // PM_REMOVE 表示取出后从队列删除。
            let ret = unsafe { w::PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, w::PM_REMOVE) };
            if ret == 0 {
                break;
            }
            handled_any = true;
            // SAFETY: msg 已由 PeekMessageW 填充。
            unsafe {
                w::TranslateMessage(&msg);
                w::DispatchMessageW(&msg);
            }
        }
        handled_any
    }

    /// 取出并清空待处理按键。
    pub fn take_keys(&mut self) -> Vec<KeyEvent> {
        if self.context.is_null() {
            return Vec::new();
        }
        // SAFETY: context 由本结构体独占所有，指向有效的 Context。
        let ctx = unsafe { &mut *self.context };
        std::mem::take(&mut ctx.keys)
    }

    /// 当前鼠标状态：`(客户区坐标 x, y, 左键是否按下, 右键是否按下)`。
    pub fn mouse_state(&mut self) -> (i32, i32, bool, bool) {
        if self.context.is_null() {
            return (0, 0, false, false);
        }
        // SAFETY: 同上。
        let ctx = unsafe { &*self.context };
        (
            ctx.mouse.0,
            ctx.mouse.1,
            ctx.mouse_left_down,
            ctx.mouse_right_down,
        )
    }

    /// 取出并清零滚轮增量（正数向上滚）。
    pub fn take_wheel(&mut self) -> i32 {
        if self.context.is_null() {
            return 0;
        }
        // SAFETY: 同上。
        let ctx = unsafe { &mut *self.context };
        std::mem::take(&mut ctx.wheel_delta)
    }

    /// 是否有用户操作请求重绘（键鼠事件）。
    pub fn take_render_request(&mut self) -> bool {
        if self.context.is_null() {
            return false;
        }
        // SAFETY: 同上。
        let ctx = unsafe { &mut *self.context };
        std::mem::take(&mut ctx.needs_render)
    }

    /// 把画布内容提交到屏幕。
    ///
    /// `dirty` 为 `None` 时提交整屏；为 `Some(r)` 时只提交该矩形
    /// （像素仍从画布的对应位置取）。矩形会被裁剪到窗口范围。
    ///
    /// 返回实际上传的像素数，便于统计"脏区重绘到底省了多少"。
    pub fn upload(&mut self, dirty: Option<Rect>) -> Result<u64, String> {
        let (w_, h_) = self.canvas.size();
        let full = Rect::new(0.0, 0.0, w_ as f32, h_ as f32);
        let region = match dirty {
            None => full,
            Some(r) => r.clamp_to(&full),
        };
        if region.w < 1.0 || region.h < 1.0 {
            // 脏区为空：不需要提交任何东西。这正是「静止时零开销」的落点。
            return Ok(0);
        }
        let x = region.x.floor() as i32;
        let y = region.y.floor() as i32;
        let cw = (region.right().ceil() as i32 - x).max(1);
        let ch = (region.bottom().ceil() as i32 - y).max(1);

        let size = w::SIZE { cx: cw, cy: ch };
        let src_pt = w::POINT { x, y };
        let dst_pt = w::POINT {
            x: self.origin.0 + x,
            y: self.origin.1 + y,
        };
        let blend = w::BLENDFUNCTION {
            BlendOp: w::AC_SRC_OVER,
            BlendFlags: 0,
            // 255 = 使用源位图的每像素 alpha（这是我们能做到抗锯齿透明的前提）
            SourceConstantAlpha: 255,
            AlphaFormat: w::AC_SRC_ALPHA,
        };

        // SAFETY: hwnd 有效；canvas 的内存 DC 与 DIB 都有效且尺寸不小于 region；
        // 三个结构体都是本栈帧内的局部变量，生命周期覆盖调用；
        // ULW_ALPHA 表示按 BLENDFUNCTION 的每像素 alpha 合成。
        let ok = unsafe {
            w::GdiFlush();
            w::UpdateLayeredWindow(
                self.hwnd,
                ptr::null_mut(),
                &dst_pt,
                &size,
                self.canvas.dc(),
                &src_pt,
                0,
                &blend,
                w::ulw::ALPHA,
            )
        };
        if ok == 0 {
            return Err("UpdateLayeredWindow 失败".to_string());
        }

        self.uploads += 1;
        self.uploaded_pixels += (cw as u64) * (ch as u64);
        self.last_upload = Some(region);
        Ok((cw as u64) * (ch as u64))
    }

    /// 上次上传的区域。
    pub fn last_upload(&self) -> Option<Rect> {
        self.last_upload
    }

    /// 累计上传次数。
    pub fn uploads(&self) -> u64 {
        self.uploads
    }

    /// 累计上传的像素数。
    pub fn uploaded_pixels(&self) -> u64 {
        self.uploaded_pixels
    }

    /// 销毁窗口。
    pub fn destroy(&mut self) {
        // SAFETY: hwnd 有效；DestroyWindow 会触发 WM_DESTROY，
        // 但我们自己在 Drop 里回收 Context，不依赖回调。
        unsafe {
            if !self.hwnd.is_null() {
                w::DestroyWindow(self.hwnd);
            }
        }
    }
}

impl Drop for LayeredWindow {
    fn drop(&mut self) {
        self.destroy();
        // 关掉窗口后，系统不会再回调 wnd_proc，可以安全回收 Context。
        // SAFETY: context 由 Box::into_raw 得到，仅在此处回收一次。
        if !self.context.is_null() {
            unsafe { drop(Box::from_raw(self.context)) };
            self.context = ptr::null_mut();
        }
    }
}

/// 窗口过程。
///
/// # 为什么必须自己处理 `ERASEBKGND`
///
/// 分层窗口如果让系统擦背景，会出现闪烁（系统先填一次背景，再被我们的
/// alpha 位图覆盖）。直接返回 1 表示"已处理"，跳过系统擦除。
///
/// # `WINDOWPOSCHANGING`
///
/// 参考实现把窗口设为整屏，但游戏切分辨率/全屏切换时窗口会被系统移动。
/// 这里阻止位置改变（HUD 自己用 [`LayeredWindow::origin`] 管理坐标），
/// 避免窗口被挪到屏幕外。
unsafe extern "system" fn wnd_proc(
    hwnd: w::HWND,
    msg: w::UINT,
    wparam: w::WPARAM,
    lparam: w::LPARAM,
) -> isize {
    // 取出窗口创建时挂上去的 Context
    let ctx_ptr = unsafe { w::GetWindowLongPtrW(hwnd, w::GWLP_USERDATA) as *mut Context };

    match msg {
        w::wm::ERASEBKGND => 1,
        w::wm::PAINT => 0,
        w::wm::CLOSE => {
            if let Some(ctx) = unsafe { ctx_ptr.as_mut() } {
                ctx.quit = true;
            }
            // 不在这里 DestroyWindow：让主循环统一处理退出，
            // 避免在回调里释放仍被主循环使用的资源。
            0
        }
        w::wm::DESTROY => {
            // 交给 post_quit 唤醒消息循环；不在这里回收 Context。
            unsafe { w::PostQuitMessage(0) };
            0
        }
        w::wm::APP_QUIT => {
            if let Some(ctx) = unsafe { ctx_ptr.as_mut() } {
                ctx.quit = true;
            }
            0
        }
        w::wm::KEYDOWN | w::wm::SYSKEYDOWN => {
            if let Some(ctx) = unsafe { ctx_ptr.as_mut() } {
                let vk = wparam as i32;
                // SAFETY: GetAsyncKeyState 可在任意线程调用。
                let ctrl = unsafe { (w::GetAsyncKeyState(w::vk::CONTROL) as u16 & 0x8000) != 0 };
                let shift = unsafe { (w::GetAsyncKeyState(w::vk::SHIFT) as u16 & 0x8000) != 0 };
                // 简单限长，避免长按导致的无限增长（理论上只有一次 KEYDOWN）
                if ctx.keys.len() < 256 {
                    ctx.keys.push(KeyEvent { vk, ctrl, shift });
                }
                ctx.needs_render = true;
                // Esc 直接请求退出，与参考实现一致
                if vk == w::vk::ESCAPE {
                    ctx.quit = true;
                }
            }
            0
        }
        w::wm::MOUSEMOVE => {
            if let Some(ctx) = unsafe { ctx_ptr.as_mut() } {
                ctx.mouse.0 = loword_signed(lparam);
                ctx.mouse.1 = hiword_signed(lparam);
            }
            0
        }
        w::wm::LBUTTONDOWN => {
            if let Some(ctx) = unsafe { ctx_ptr.as_mut() } {
                ctx.mouse_left_down = true;
                ctx.mouse.0 = loword_signed(lparam);
                ctx.mouse.1 = hiword_signed(lparam);
                ctx.needs_render = true;
                // 拖拽时捕获鼠标，确保移出窗口也能收到 MOUSEMOVE / LBUTTONUP
                unsafe { w::SetCapture(hwnd) };
            }
            0
        }
        w::wm::LBUTTONUP => {
            if let Some(ctx) = unsafe { ctx_ptr.as_mut() } {
                ctx.mouse_left_down = false;
                ctx.mouse.0 = loword_signed(lparam);
                ctx.mouse.1 = hiword_signed(lparam);
                ctx.needs_render = true;
                unsafe { w::ReleaseCapture() };
            }
            0
        }
        w::wm::RBUTTONDOWN => {
            if let Some(ctx) = unsafe { ctx_ptr.as_mut() } {
                ctx.mouse_right_down = true;
                ctx.needs_render = true;
            }
            0
        }
        w::wm::RBUTTONUP => {
            if let Some(ctx) = unsafe { ctx_ptr.as_mut() } {
                ctx.mouse_right_down = false;
                ctx.needs_render = true;
            }
            0
        }
        w::wm::MOUSEWHEEL => {
            if let Some(ctx) = unsafe { ctx_ptr.as_mut() } {
                // 高 16 位是有符号的滚轮增量，单位是 120 的倍数
                let delta = ((wparam >> 16) & 0xFFFF) as u16 as i16 as i32;
                ctx.wheel_delta += delta;
                ctx.needs_render = true;
            }
            0
        }
        _ => unsafe { w::DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// 从 `lParam` 低 16 位取有符号坐标。
fn loword_signed(v: w::LPARAM) -> i32 {
    (v as u16) as i16 as i32
}

/// 从 `lParam` 高 16 位取有符号坐标。
fn hiword_signed(v: w::LPARAM) -> i32 {
    ((v >> 16) as u16) as i16 as i32
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use crate::gauge::Rect;

    #[test]
    fn 坐标拆解处理负数() {
        // 多显示器下鼠标坐标可能为负，必须按有符号 16 位解释
        // x = -100, y = -200
        let x: i16 = -100;
        let y: i16 = -200;
        let lparam = ((y as u16 as isize) << 16) | (x as u16 as isize);
        assert_eq!(loword_signed(lparam), -100);
        assert_eq!(hiword_signed(lparam), -200);

        // 正数
        let lparam2 = ((300isize) << 16) | 400isize;
        assert_eq!(loword_signed(lparam2), 400);
        assert_eq!(hiword_signed(lparam2), 300);
    }

    #[test]
    fn 全屏选项在极端情况下有兜底() {
        // 真实调用 GetSystemMetrics；只要不为 0 且覆盖主屏即可
        let o = WindowOptions::full_virtual_screen();
        assert!(o.width >= 640, "宽度应有兜底下限, 实际 {}", o.width);
        assert!(o.height >= 480, "高度应有兜底下限, 实际 {}", o.height);
    }

    #[test]
    fn 按键事件携带修饰键() {
        let e = KeyEvent {
            vk: 0x46,
            ctrl: true,
            shift: false,
        };
        assert_eq!(e.vk, 0x46);
        assert!(e.ctrl);
        assert!(!e.shift);
    }

    #[test]
    fn 脏区裁剪逻辑与上传范围一致() {
        // 脏区超出画布时应被裁到画布内（与 Canvas::fill_rect 同样的裁剪规则）
        let canvas = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let r = Rect::new(1800.0, 1000.0, 400.0, 400.0).clamp_to(&canvas);
        assert_eq!(r.x, 1800.0);
        assert_eq!(r.y, 1000.0);
        assert_eq!(r.w, 120.0);
        assert_eq!(r.h, 80.0);

        // 完全在外的脏区裁剪后尺寸为 0，调用方应据此跳过上传
        let gone = Rect::new(5000.0, 5000.0, 10.0, 10.0).clamp_to(&canvas);
        assert!(gone.w < 1.0 && gone.h < 1.0);
    }
}
