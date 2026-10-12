//! 手写的 Win32 绑定（user32 / gdi32）。
//!
//! # 为什么不用 `windows` crate
//!
//! `windows` crate 会自动生成全部 Win32 API 的绑定，但代价是：
//!
//! * 编译期与产物体积显著增长（本项目目标是**低资源占用**，release 产物
//!   目前 260–313 KB，引入它会大一个数量级）；
//! * 依赖树变深，而开源后别人 `cargo build` 要下载的 crate 数量直接影响体验。
//!
//! 本项目真正需要的 API 只有二十几个，全部是**长期稳定的 Win32 接口**
//! （`CreateWindowExW`、`UpdateLayeredWindow`、`BitBlt`…这些从 Windows 2000
//! 起签名就没变过）。因此手写绑定是划算的。
//!
//! # 安全约定
//!
//! 本模块刻意保持**极薄**：只声明类型与 `extern "system"` 签名，
//! 不含任何逻辑。所有调用点都在 [`crate::window`] 里，并集中标注
//! `// SAFETY:` 说明。这样"不安全代码"的审查面被压到最小。

#![allow(non_snake_case, non_camel_case_types, dead_code)]

use std::ffi::c_void;

// ---- 基础整数类型（跟随 Windows SDK 的定义）----
/// 32 位布尔（Win32 `BOOL`）。
pub type BOOL = i32;
/// 32 位无符号整数（Win32 `UINT`）。
pub type UINT = u32;
/// 32 位有符号整数（Win32 `INT`）。
pub type INT = i32;
/// 16 位无符号整数（Win32 `WORD`）。
pub type WORD = u16;
/// 32 位无符号长整数（Win32 `DWORD`）。
pub type DWORD = u32;
/// 32 位有符号长整数（Win32 `LONG`）。
pub type LONG = i32;
/// 字节（Win32 `BYTE`）。
pub type BYTE = u8;
/// 指针大小的无符号整数（Win32 `WPARAM`）。
pub type WPARAM = usize;
/// 指针大小的有符号整数（Win32 `LPARAM`）。
pub type LPARAM = isize;
/// 句柄基类型。
pub type HANDLE = *mut c_void;
/// 窗口句柄。
pub type HWND = *mut c_void;
/// 设备上下文句柄。
pub type HDC = *mut c_void;
/// GDI 对象句柄（画笔、画刷、字体、位图都是它）。
pub type HGDIOBJ = *mut c_void;
/// 菜单句柄。
pub type HMENU = *mut c_void;
/// 模块句柄。
pub type HINSTANCE = *mut c_void;
/// 图标句柄。
pub type HICON = *mut c_void;
/// 光标句柄。
pub type HCURSOR = *mut c_void;
/// 画刷句柄。
pub type HBRUSH = *mut c_void;
/// 画笔句柄。
pub type HPEN = *mut c_void;
/// 字体句柄。
pub type HFONT = *mut c_void;
/// 位图句柄。
pub type HBITMAP = *mut c_void;
/// 窗口类原子。
pub type ATOM = WORD;
/// 窗口类的 `hCursor` 特殊值：不设置光标。
pub const IDC_ARROW: usize = 32512;

/// 在 [`wnd_proc`] 返回：已处理该消息。
pub const LRESULT_HANDLED: isize = 0;

/// GDI 颜色值（`0x00BBGGRR`）。
pub type COLORREF = DWORD;

/// 把 RGB 分量组成 `COLORREF`。
pub const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

/// 光栅操作码：直接复制源。
pub const SRCCOPY: DWORD = 0x00CC_0020;

// ============================================================================
// 结构体
// ============================================================================

/// 一个点。
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct POINT {
    /// X 坐标。
    pub x: LONG,
    /// Y 坐标。
    pub y: LONG,
}

/// 尺寸。
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SIZE {
    /// 宽。
    pub cx: LONG,
    /// 高。
    pub cy: LONG,
}

/// 矩形。
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RECT {
    /// 左。
    pub left: LONG,
    /// 上。
    pub top: LONG,
    /// 右。
    pub right: LONG,
    /// 下。
    pub bottom: LONG,
}

/// 位图信息头（`BITMAPINFOHEADER`）。`biHeight` 为负表示**自上而下**的位图，
/// 这样我们可以在缓冲区里按「第 0 行是屏幕最上方」的顺序写像素，更符合直觉。
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct BITMAPINFOHEADER {
    /// 本结构大小。
    pub biSize: DWORD,
    /// 宽。
    pub biWidth: LONG,
    /// 高（负值 = 自上而下）。
    pub biHeight: LONG,
    /// 位平面数，固定 1。
    pub biPlanes: WORD,
    /// 每像素位数。
    pub biBitCount: WORD,
    /// 压缩方式，`BI_RGB` = 0。
    pub biCompression: DWORD,
    /// 图像数据大小（`BI_RGB` 时可为 0）。
    pub biSizeImage: DWORD,
    /// 水平分辨率。
    pub biXPelsPerMeter: LONG,
    /// 垂直分辨率。
    pub biYPelsPerMeter: LONG,
    /// 调色板颜色数。
    pub biClrUsed: DWORD,
    /// 重要颜色数。
    pub biClrImportant: DWORD,
}

/// 包含调色板的位图信息。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct BITMAPINFO {
    /// 信息头。
    pub bmiHeader: BITMAPINFOHEADER,
    /// 调色板首项（32 位图用不到，但结构上必须有）。
    pub bmiColors: [DWORD; 1],
}

impl BITMAPINFO {
    /// 为「自上而下、32 位、`BI_RGB`」的位图构造信息结构。
    pub fn top_down_32bpp(width: i32, height: i32) -> Self {
        Self {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as DWORD,
                biWidth: width,
                // 负高度 = 自上而下
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0, // BI_RGB
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [0],
        }
    }
}

/// 透明度混合方式（`BLENDFUNCTION`）。
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct BLENDFUNCTION {
    /// 固定为 `AC_SRC_OVER` = 0。
    pub BlendOp: BYTE,
    /// 固定 0。
    pub BlendFlags: BYTE,
    /// 源位图整体 alpha（255 = 按每像素 alpha）。
    pub SourceConstantAlpha: BYTE,
    /// `AC_SRC_ALPHA` = 1（使用每像素 alpha）。
    pub AlphaFormat: BYTE,
}

/// `AC_SRC_OVER`。
pub const AC_SRC_OVER: BYTE = 0;
/// 使用源位图的每像素 alpha。
pub const AC_SRC_ALPHA: BYTE = 1;

/// `UpdateLayeredWindow` 的 `dwFlags` 位。
pub mod ulw {
    /// 使用每像素 alpha（`BLENDFUNCTION`）。
    pub const ALPHA: u32 = 0x0000_0002;
    /// 使用源 DC 作为图像来源。
    pub const OPACITY: u32 = 0x0000_0004;
}

/// 窗口消息。
pub mod wm {
    /// 窗口销毁。
    pub const DESTROY: super::UINT = 0x0002;
    /// 窗口大小改变。
    pub const SIZE: super::UINT = 0x0005;
    /// 重绘请求（我们不处理，因为主动渲染）。
    pub const PAINT: super::UINT = 0x000F;
    /// 擦除背景（必须返回 1 阻止系统擦除，否则分层窗口会闪）。
    pub const ERASEBKGND: super::UINT = 0x0014;
    /// 窗口位置/大小即将改变。
    pub const WINDOWPOSCHANGING: super::UINT = 0x0046;
    /// 按键按下。
    pub const KEYDOWN: super::UINT = 0x0100;
    /// 按键抬起。
    pub const KEYUP: super::UINT = 0x0101;
    /// 系统按键按下（含 Alt 组合）。
    pub const SYSKEYDOWN: super::UINT = 0x0104;
    /// 系统按键抬起。
    pub const SYSKEYUP: super::UINT = 0x0105;
    /// 鼠标移动。
    pub const MOUSEMOVE: super::UINT = 0x0200;
    /// 左键按下。
    pub const LBUTTONDOWN: super::UINT = 0x0201;
    /// 左键抬起。
    pub const LBUTTONUP: super::UINT = 0x0202;
    /// 右键按下。
    pub const RBUTTONDOWN: super::UINT = 0x0204;
    /// 右键抬起。
    pub const RBUTTONUP: super::UINT = 0x0205;
    /// 鼠标滚轮。
    pub const MOUSEWHEEL: super::UINT = 0x020A;
    /// 窗口即将关闭。
    pub const CLOSE: super::UINT = 0x0010;
    /// 定时器。
    pub const TIMER: super::UINT = 0x0113;
    /// 自定义消息：请求渲染（由渲染线程投递到窗口线程）。
    pub const APP_RENDER: super::UINT = 0x0400 + 1;
    /// 自定义消息：请求退出。
    pub const APP_QUIT: super::UINT = 0x0400 + 2;
}

/// 窗口样式。
pub mod ws {
    /// 子窗口/弹出窗口（无标题栏）。分层窗口必须用这个。
    pub const POPUP: super::DWORD = 0x8000_0000;
    /// 初始可见。
    pub const VISIBLE: super::DWORD = 0x1000_0000;
    /// 裁剪同级窗口。
    pub const CLIPSIBLINGS: super::DWORD = 0x0400_0000;
}
/// 别名，便于按语义书写。
pub use ws as ws_popup;

/// 扩展窗口样式。
pub mod ws_ex {
    /// 分层窗口（支持每像素 alpha）。
    pub const LAYERED: super::DWORD = 0x0008_0000;
    /// **鼠标事件穿透**。关键：让点击落到游戏窗口而不是 HUD 上。
    pub const TRANSPARENT: super::DWORD = 0x0000_0020;
    /// 总在最前。
    pub const TOPMOST: super::DWORD = 0x0000_0008;
    /// 工具窗口：不在任务栏显示、不参与 Alt+Tab。
    pub const TOOLWINDOW: super::DWORD = 0x0000_0080;
    /// 不接受焦点（避免抢走游戏的键盘输入）。
    pub const NOACTIVATE: super::DWORD = 0x0800_0000;
}

/// `WM_SYSCOMMAND` 的 `wParam`：屏蔽屏保与显示器休眠。
pub const SC_SCREENSAVE: WPARAM = 0xF140;
/// 屏蔽显示器省电。
pub const SC_MONITORPOWER: WPARAM = 0xF170;

/// 窗口类的 `hbrBackground` 特殊值：不绘制背景。
pub const BLACK_BRUSH: usize = 4;

/// `GetWindowLongPtrW` 的索引：窗口用户数据（我们自己挂的 Context 指针）。
pub const GWLP_USERDATA: INT = -21;

/// `SetWindowPos` 的 `hWndInsertAfter`：置于最顶层。
pub const HWND_TOPMOST: HWND = -1isize as HWND;
/// `SetWindowPos` 标志。
pub mod swp {
    /// 不改变大小与位置，只改 Z 序。
    pub const NOSIZE: super::UINT = 0x0001;
    /// 不移动。
    pub const NOMOVE: super::UINT = 0x0002;
    /// 不激活窗口（不抢焦点）。
    pub const NOACTIVATE: super::UINT = 0x0010;
    /// 显示窗口。
    pub const SHOWWINDOW: super::UINT = 0x0040;
}

/// 虚拟键码。
pub mod vk {
    /// Shift（左右任一）。
    pub const SHIFT: i32 = 0x10;
    /// Ctrl（左右任一）。
    pub const CONTROL: i32 = 0x11;
    /// Alt（左右任一）。
    pub const MENU: i32 = 0x12;
    /// Esc。
    pub const ESCAPE: i32 = 0x1B;
    /// F1。
    pub const F1: i32 = 0x70;
    /// F2。
    pub const F2: i32 = 0x71;
    /// F5。
    pub const F5: i32 = 0x74;
    /// F9。
    pub const F9: i32 = 0x78;
    /// F11。
    pub const F11: i32 = 0x7A;
    /// F12。
    pub const F12: i32 = 0x7B;
    /// Insert。
    pub const INSERT: i32 = 0x2D;
    /// Home。
    pub const HOME: i32 = 0x24;
    /// End。
    pub const END: i32 = 0x23;
    /// PageUp。
    pub const PRIOR: i32 = 0x21;
    /// PageDown。
    pub const NEXT: i32 = 0x22;
    /// 上箭头。
    pub const UP: i32 = 0x26;
    /// 下箭头。
    pub const DOWN: i32 = 0x28;
    /// 左箭头。
    pub const LEFT: i32 = 0x25;
    /// 右箭头。
    pub const RIGHT: i32 = 0x27;
    /// 减号。
    pub const SUBTRACT: i32 = 0x6D;
    /// 加号。
    pub const ADD: i32 = 0x6B;
    /// 数字 1。
    pub const KEY_1: i32 = 0x31;
    /// 数字 2。
    pub const KEY_2: i32 = 0x32;
    /// 数字 3。
    pub const KEY_3: i32 = 0x33;
    /// 数字 4。
    pub const KEY_4: i32 = 0x34;
    /// 数字 5。
    pub const KEY_5: i32 = 0x35;
    /// 0。
    pub const KEY_0: i32 = 0x30;
    /// S。
    pub const S: i32 = 0x53;
    /// C。
    pub const C: i32 = 0x43;
    /// H。
    pub const H: i32 = 0x48;
}

// ============================================================================
// 字体与文本
// ============================================================================

/// 空画刷（`GetStockObject`）。
///
/// `Rectangle` / `Ellipse` 会用**当前画刷**填充内部；画空心边框时必须先选入
/// 空画刷，否则内部会被填充（把仪表糊成一块）。
pub const NULL_BRUSH: usize = 5;

/// 固定间距字体（`GetStockObject`）—— 用于调试文本，避免加载字体的麻烦。
pub const ANSI_FIXED_FONT: usize = 11;
/// 系统字体。
pub const DEFAULT_GUI_FONT: usize = 17;

/// 创建字体时的字重。
pub mod fw {
    /// 常规。
    pub const NORMAL: super::INT = 400;
    /// 半粗。
    pub const SEMIBOLD: super::INT = 600;
    /// 粗体。
    pub const BOLD: super::INT = 700;
    /// 极粗。
    pub const BLACK: super::INT = 900;
}

/// 文本对齐标志。
pub mod dt {
    /// 左对齐。
    pub const LEFT: super::UINT = 0x0000_0000;
    /// 居中。
    pub const CENTER: super::UINT = 0x0000_0001;
    /// 右对齐。
    pub const RIGHT: super::UINT = 0x0000_0002;
    /// 垂直居中。
    pub const V_CENTER: super::UINT = 0x0000_0004;
    /// 单行（不换行）。
    pub const SINGLELINE: super::UINT = 0x0000_0020;
    /// 不裁剪（允许文字超出矩形）。
    pub const NOCLIP: super::UINT = 0x0001_0000;
}

/// 画笔样式。
pub mod ps {
    /// 实线。
    pub const SOLID: super::INT = 0;
    /// 不画线。
    pub const NULL: super::INT = 5;
}

/// 背景模式：透明（文字不带底色）。
pub const TRANSPARENT_BK: INT = 1;

// ============================================================================
// 外部函数声明
// ============================================================================

/// 窗口过程函数签名。
pub type WNDPROC = Option<unsafe extern "system" fn(HWND, UINT, WPARAM, LPARAM) -> isize>;

/// 窗口类定义（`WNDCLASSEXW`）。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct WNDCLASSEXW {
    /// 结构大小。
    pub cbSize: UINT,
    /// 类样式。
    pub style: UINT,
    /// 窗口过程。
    pub lpfnWndProc: WNDPROC,
    /// 类的额外字节数。
    pub cbClsExtra: INT,
    /// 窗口的额外字节数。
    pub cbWndExtra: INT,
    /// 实例句柄。
    pub hInstance: HINSTANCE,
    /// 图标。
    pub hIcon: HICON,
    /// 光标。
    pub hCursor: HCURSOR,
    /// 背景画刷。
    pub hbrBackground: HBRUSH,
    /// 菜单名。
    pub lpszMenuName: *const u16,
    /// 类名。
    pub lpszClassName: *const u16,
    /// 小图标。
    pub hIconSm: HICON,
}

impl Default for WNDCLASSEXW {
    fn default() -> Self {
        Self {
            cbSize: std::mem::size_of::<Self>() as UINT,
            style: 0,
            lpfnWndProc: None,
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: std::ptr::null_mut(),
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: std::ptr::null(),
            hIconSm: std::ptr::null_mut(),
        }
    }
}

/// 消息结构（`MSG`）。
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct MSG {
    /// 窗口句柄。
    pub hwnd: HWND,
    /// 消息 ID。
    pub message: UINT,
    /// 附加参数。
    pub wParam: WPARAM,
    /// 附加参数。
    pub lParam: LPARAM,
    /// 消息时间（`DWORD`）。
    pub time: DWORD,
    /// 消息产生时的光标位置。
    pub pt: POINT,
}

/// 批量声明外部函数的宏。
///
/// 之所以要这样一个宏：Win32 需要声明的函数有几十个，手写 `extern "system"`
/// 块会导致大量重复；宏把「函数名 + 形参名与类型 + 返回类型」一次写完。
///
/// 注意形参必须写成 `名: 类型`，宏会原样透传（**不能**只捕类型，
/// 否则生成 `fn f(HDC, ...)` 这种非法签名）。
macro_rules! extern_fns {
    ($($fn_name:ident($($pname:ident : $pty:ty),* $(,)?) -> $ret:ty;)*) => {
        extern "system" {
            $(
                #[link_name = stringify!($fn_name)]
                #[allow(missing_docs)] // 这些是 Win32 官方 API 的直译，语义见 MSDN
                pub fn $fn_name($($pname: $pty),*) -> $ret;
            )*
        }
    };
}

extern_fns! {
    // ---- user32：窗口 ----
    GetModuleHandleW(lpModuleName: *const u16) -> HINSTANCE;
    RegisterClassExW(lpwcx: *const WNDCLASSEXW) -> ATOM;
    CreateWindowExW(
        dwExStyle: DWORD, lpClassName: *const u16, lpWindowName: *const u16,
        dwStyle: DWORD, X: INT, Y: INT, nWidth: INT, nHeight: INT,
        hWndParent: HWND, hMenu: HMENU, hInstance: HINSTANCE, lpParam: *mut c_void
    ) -> HWND;
    DestroyWindow(hWnd: HWND) -> BOOL;
    DefWindowProcW(hWnd: HWND, Msg: UINT, wParam: WPARAM, lParam: LPARAM) -> isize;
    GetMessageW(lpMsg: *mut MSG, hWnd: HWND, wMsgFilterMin: UINT, wMsgFilterMax: UINT) -> BOOL;
    PeekMessageW(
        lpMsg: *mut MSG, hWnd: HWND, wMsgFilterMin: UINT, wMsgFilterMax: UINT,
        wRemoveMsg: UINT
    ) -> BOOL;
    TranslateMessage(lpMsg: *const MSG) -> BOOL;
    DispatchMessageW(lpMsg: *const MSG) -> isize;
    PostMessageW(hWnd: HWND, Msg: UINT, wParam: WPARAM, lParam: LPARAM) -> BOOL;
    PostQuitMessage(nExitCode: INT) -> ();
    SetWindowPos(
        hWnd: HWND, hWndInsertAfter: HWND, X: INT, Y: INT,
        cx: INT, cy: INT, uFlags: UINT
    ) -> BOOL;
    GetSystemMetrics(nIndex: INT) -> INT;
    SetCapture(hWnd: HWND) -> HWND;
    ReleaseCapture() -> BOOL;
    GetCursorPos(lpPoint: *mut POINT) -> BOOL;
    ScreenToClient(hWnd: HWND, lpPoint: *mut POINT) -> BOOL;
    GetAsyncKeyState(vKey: INT) -> i16;
    SetTimer(hWnd: HWND, nIDEvent: WPARAM, uElapse: UINT, lpTimerFunc: *mut c_void) -> WPARAM;
    KillTimer(hWnd: HWND, uIDEvent: WPARAM) -> BOOL;
    GetWindowLongPtrW(hWnd: HWND, nIndex: INT) -> isize;
    SetWindowLongPtrW(hWnd: HWND, nIndex: INT, dwNewLong: isize) -> isize;
    UpdateLayeredWindow(
        hWnd: HWND, hdcDst: HDC, pptDst: *const POINT, psize: *const SIZE,
        hdcSrc: HDC, pptSrc: *const POINT, crKey: COLORREF,
        pblend: *const BLENDFUNCTION, dwFlags: DWORD
    ) -> BOOL;

    // ---- gdi32：绘制 ----
    GetStockObject(i: INT) -> HGDIOBJ;
    GetDC(hWnd: HWND) -> HDC;
    ReleaseDC(hWnd: HWND, hDC: HDC) -> INT;
    CreateCompatibleDC(hdc: HDC) -> HDC;
    DeleteDC(hdc: HDC) -> BOOL;
    CreateDIBSection(
        hdc: HDC, pbmi: *const BITMAPINFO, usage: UINT,
        ppvBits: *mut *mut c_void, hSection: HANDLE, offset: DWORD
    ) -> HBITMAP;
    SelectObject(hdc: HDC, h: HGDIOBJ) -> HGDIOBJ;
    DeleteObject(ho: HGDIOBJ) -> BOOL;
    CreateSolidBrush(color: COLORREF) -> HBRUSH;
    CreatePen(fnPenStyle: INT, nWidth: INT, color: COLORREF) -> HPEN;
    CreateFontW(
        cHeight: INT, cWidth: INT, cEscapement: INT, cOrientation: INT,
        cWeight: INT, bItalic: DWORD, bUnderline: DWORD, bStrikeOut: DWORD,
        iCharSet: DWORD, iOutPrecision: DWORD, iClipPrecision: DWORD,
        iQuality: DWORD, iPitchAndFamily: DWORD, pszFaceName: *const u16
    ) -> HFONT;
    SetBkMode(hdc: HDC, mode: INT) -> INT;
    SetTextColor(hdc: HDC, color: COLORREF) -> COLORREF;
    SetBkColor(hdc: HDC, color: COLORREF) -> COLORREF;
    TextOutW(hdc: HDC, x: INT, y: INT, lpString: *const u16, c: INT) -> BOOL;
    DrawTextW(
        hdc: HDC, lpchText: *const u16, cchText: INT,
        lprc: *mut RECT, format: UINT
    ) -> INT;
    Rectangle(hdc: HDC, left: INT, top: INT, right: INT, bottom: INT) -> BOOL;
    Ellipse(hdc: HDC, left: INT, top: INT, right: INT, bottom: INT) -> BOOL;
    MoveToEx(hdc: HDC, x: INT, y: INT, lppt: *mut POINT) -> BOOL;
    LineTo(hdc: HDC, x: INT, y: INT) -> BOOL;
    Polygon(hdc: HDC, points: *const POINT, count: INT) -> BOOL;
    BitBlt(
        hdcDest: HDC, x: INT, y: INT, w: INT, h: INT,
        hdcSrc: HDC, x1: INT, y1: INT, rop: DWORD
    ) -> BOOL;
    PatBlt(hdc: HDC, x: INT, y: INT, w: INT, h: INT, rop: DWORD) -> BOOL;
    GdiFlush() -> BOOL;
}

/// `PatBlt` 光栅码：用画刷填充。
pub const PATCOPY: DWORD = 0x00F0_0021;
/// `GetSystemMetrics` 索引：屏幕宽。
pub const SM_CXSCREEN: INT = 0;
/// `GetSystemMetrics` 索引：屏幕高。
pub const SM_CYSCREEN: INT = 1;
/// `GetSystemMetrics` 索引：虚拟屏幕（多显示器总区域）左边界。
pub const SM_XVIRTUALSCREEN: INT = 76;
/// `GetSystemMetrics` 索引：虚拟屏幕顶边界。
pub const SM_YVIRTUALSCREEN: INT = 77;
/// `GetSystemMetrics` 索引：虚拟屏幕总宽。
pub const SM_CXVIRTUALSCREEN: INT = 78;
/// `GetSystemMetrics` 索引：虚拟屏幕总高。
pub const SM_CYVIRTUALSCREEN: INT = 79;

/// `PeekMessage` 的 `wRemoveMsg`。
/// 取出消息。
pub const PM_REMOVE: UINT = 0x0001;

/// 把 Rust 字符串转成以 NUL 结尾的 UTF-16，供 Win32 宽字符接口使用。
pub fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 结构体大小与Win32约定一致() {
        // 这些大小若不对，传指针给系统 API 会读到错误的偏移，
        // 表现为窗口创建失败或绘制错乱 —— 且很难定位。
        assert_eq!(std::mem::size_of::<POINT>(), 8);
        assert_eq!(std::mem::size_of::<SIZE>(), 8);
        assert_eq!(std::mem::size_of::<RECT>(), 16);
        assert_eq!(std::mem::size_of::<BITMAPINFOHEADER>(), 40);
        assert_eq!(std::mem::size_of::<BLENDFUNCTION>(), 4);
        assert_eq!(std::mem::size_of::<WNDCLASSEXW>(), 80);
        // MSG 在两个指针大小平台上分别是 48 / 40 字节；这里是 64 位。
        assert_eq!(std::mem::size_of::<MSG>(), 48);
        assert_eq!(std::mem::size_of::<BITMAPINFO>(), 44);
    }

    #[test]
    fn 位上而下位图的高度为负() {
        let bi = BITMAPINFO::top_down_32bpp(1920, 1080);
        assert_eq!(bi.bmiHeader.biWidth, 1920);
        assert_eq!(
            bi.bmiHeader.biHeight, -1080,
            "负高度表示自上而下，这样缓冲区第 0 行就是屏幕最上方"
        );
        assert_eq!(bi.bmiHeader.biBitCount, 32);
        assert_eq!(bi.bmiHeader.biPlanes, 1);
        assert_eq!(bi.bmiHeader.biCompression, 0, "BI_RGB");
        assert_eq!(
            bi.bmiHeader.biSize as usize,
            std::mem::size_of::<BITMAPINFOHEADER>()
        );
    }

    #[test]
    fn colorref按bbggrr排列() {
        // COLORREF 是 0x00BBGGRR，与最常见的 RGB 顺序相反
        assert_eq!(rgb(0xFF, 0x00, 0x00), 0x0000_00FF); // 红
        assert_eq!(rgb(0x00, 0xFF, 0x00), 0x0000_FF00); // 绿
        assert_eq!(rgb(0x00, 0x00, 0xFF), 0x00FF_0000); // 蓝
        assert_eq!(rgb(0x11, 0x22, 0x33), 0x0033_2211);
    }

    #[test]
    fn 宽字符串以NUL结尾() {
        let w = to_wide("FH6");
        assert_eq!(w, vec![0x46, 0x48, 0x36, 0]);
        assert_eq!(to_wide(""), vec![0]);

        // 中文字符也应正确编码为 UTF-16
        let w2 = to_wide("地平线");
        assert_eq!(w2.len(), 4); // 3 个字符 + NUL
        assert_eq!(w2[3], 0);
        assert_eq!(w2[0], '地' as u16);
    }

    #[test]
    fn 关键样式位不冲突且取值正确() {
        // 这些常量写错会导致窗口行为完全不对（例如挡住游戏鼠标）
        assert_eq!(ws_ex::TRANSPARENT, 0x20);
        assert_eq!(ws_ex::LAYERED, 0x0008_0000);
        assert_eq!(ws_ex::TOPMOST, 0x0000_0008);
        assert_eq!(ws_ex::TOOLWINDOW, 0x0000_0080);
        assert_eq!(ws_ex::NOACTIVATE, 0x0800_0000);
        // 组合后不应互相覆盖
        let combo = ws_ex::LAYERED
            | ws_ex::TRANSPARENT
            | ws_ex::TOPMOST
            | ws_ex::TOOLWINDOW
            | ws_ex::NOACTIVATE;
        assert_eq!(combo.count_ones(), 5);
    }

    #[test]
    fn 窗口类大小字段自动填对() {
        let wc = WNDCLASSEXW::default();
        assert_eq!(wc.cbSize as usize, std::mem::size_of::<WNDCLASSEXW>());
        assert_eq!(wc.style, 0);
        assert_eq!(wc.hbrBackground, std::ptr::null_mut());
    }

    #[test]
    fn 混合结构字段取值符合规范() {
        let bf = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA,
        };
        assert_eq!(bf.BlendOp, 0);
        assert_eq!(bf.BlendFlags, 0);
        assert_eq!(bf.SourceConstantAlpha, 255, "255 表示按每像素 alpha");
        assert_eq!(bf.AlphaFormat, 1);
    }
}
