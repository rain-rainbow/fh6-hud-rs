//! 单实例保护。
//!
//! # 为什么需要
//!
//! 关闭窗口默认是「收到托盘」而不是退出。用户看不到托盘图标（Win10 会把新图标
//! 收进任务栏的 `^` 折叠区）时会以为程序已经关了，于是再双击一次 —— 结果又起
//! 一个实例。实测中用户连续开了 3 个，每个都可能在后台持有 WebView2 数据目录。
//!
//! 因此这里做两件事：
//!
//! 1. 用**命名互斥量**保证同一时刻只有一个实例；
//! 2. 第二次启动时，把**已有实例的窗口唤到前台**，让用户看到「它一直都在」。
//!
//! # 为什么不用 tauri-plugin-single-instance
//!
//! 该插件在 crates.io 上只发布了 `3.0.0-alpha`（对应 Tauri 3），与当前 Tauri 2
//! 不匹配。而这里需要的功能只有「互斥 + 找窗口」，用 Win32 三个函数即可，
//! 不必为此引入额外依赖。
//!
//! # unsafe 的边界
//!
//! 本文件是全 crate **唯一**允许 `unsafe` 的地方（crate 级用 `deny` 而非
//! `forbid`），且只包含 4 个 Win32 函数的声明与调用，不涉及任何手工内存管理：
//! 互斥量句柄在整个进程生命周期内故意不释放（`mem::forget`），由系统在进程
//! 退出时回收。

#![allow(unsafe_code)]

use std::ffi::c_void;

type Handle = *mut c_void;
type Bool = i32;

/// `SW_RESTORE`：把最小化/隐藏的窗口恢复显示。
const SW_RESTORE: i32 = 9;

/// 互斥量名称。带 `Local\` 前缀：只在当前登录会话内生效
/// （多用户同时登录时互不干扰）。
const MUTEX_NAME: &str = "Local\\fh6-hud-config-gui-singleton";

#[link(name = "kernel32")]
extern "system" {
    fn CreateMutexW(
        attributes: *mut c_void,
        initial_owner: Bool,
        name: *const u16,
    ) -> Handle;
    fn GetLastError() -> u32;
}

#[link(name = "user32")]
extern "system" {
    fn FindWindowW(class: *const u16, title: *const u16) -> Handle;
    fn ShowWindow(handle: Handle, cmd: i32) -> Bool;
    fn SetForegroundWindow(handle: Handle) -> Bool;
}

/// 把 Rust 字符串转成以 NUL 结尾的 UTF-16。
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Windows 的 `ERROR_ALREADY_EXISTS`。
const ERROR_ALREADY_EXISTS: u32 = 183;

/// 尝试成为唯一实例。
///
/// * 返回 `true`：本进程是第一个实例，可以继续启动；
/// * 返回 `false`：已有实例在运行，调用方应**立即退出**（并已尝试唤回其窗口）。
pub fn acquire() -> bool {
    let name = wide(MUTEX_NAME);
    // SAFETY: 只传一个合法的 UTF-16 指针与空属性指针，无内存所有权转移。
    let handle = unsafe { CreateMutexW(std::ptr::null_mut(), 0, name.as_ptr()) };
    if handle.is_null() {
        // 创建失败（极端情况）：不阻止启动，宁可多开也不要打不开
        return true;
    }
    // SAFETY: 无参数、无副作用。
    let err = unsafe { GetLastError() };
    if err == ERROR_ALREADY_EXISTS {
        focus_existing_window();
        return false;
    }
    // 有意**不**调用 CloseHandle：句柄保持打开，互斥量一直存在到进程退出，
    // 由系统回收。（句柄是个裸指针，丢弃它不会关闭内核对象 —— 只有 CloseHandle 会。）
    let _ = handle;
    true
}

/// 把已有实例的窗口显示并置前。
///
/// 通过窗口标题查找。标题来自 `tauri.conf.json` 的 `app.windows[].title`，
/// 两者必须一致 —— 改标题时记得同步这里。
fn focus_existing_window() {
    let title = wide("FH6 HUD 配置");
    // SAFETY: 传入合法的 NUL 结尾字符串指针；返回值为窗口句柄，仅用于后续调用。
    let hwnd = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
    if hwnd.is_null() {
        return;
    }
    // SAFETY: 句柄由 FindWindowW 返回，属于本进程可访问的窗口。
    unsafe {
        ShowWindow(hwnd, SW_RESTORE);
        SetForegroundWindow(hwnd);
    }
}
