//! 用 `ShellExecuteW` 打开文件或目录。
//!
//! # 为什么不用 `Command::new("explorer")` 或 `cmd /c start`
//!
//! 两条路实测都失败过，而且失败方式各不相同：
//!
//! | 方式 | 实际表现 |
//! |---|---|
//! | `Command::new("explorer").arg(dir)` | 新 explorer 以 `0xc0000142`（DLL 初始化失败）退出并弹「应用程序无法正常启动」——已有 shell 在运行时，新起的 explorer 需要走「转交给现有 shell」的路径 |
//! | `cmd /c start "" "<dir>"` | 会额外弹出一个**黑色控制台窗口**；且从没有控制台的 GUI 进程里调用时，`start` 报「Windows 无法访问指定设备、路径或文件」 |
//!
//! `ShellExecuteW` 是这两件事的**共同底层**，直接调用它就没有中间进程：
//! 不创建控制台、不新起 explorer，还能拿到明确的错误码（`> 32` 为成功）。
//!
//! # unsafe 边界
//!
//! 与 `single_instance.rs` 一样，本文件只有 Win32 函数声明与一次调用，
//! 不涉及任何手工内存管理；crate 级 lint 为 `deny(unsafe_code)`，此处单独 allow。

#![allow(unsafe_code)]

use std::ffi::c_void;
use std::path::Path;

type Handle = *mut c_void;

/// `SW_SHOWNORMAL`
const SW_SHOWNORMAL: i32 = 1;

/// `ShellExecuteW` 约定：返回值 > 32 表示成功，否则是错误码。
const SUCCESS_THRESHOLD: isize = 32;

#[link(name = "ole32")]
extern "system" {
    /// 初始化当前线程的 COM。`ShellExecute` 解析 "open" 动作时会用到 shell COM，
    /// 在未初始化 COM 的线程上会直接失败（PowerShell 能成功是因为它早就初始化过）。
    fn CoInitializeEx(reserved: *mut c_void, model: u32) -> i32;
    /// 撤销本线程的 COM 初始化（只在该线程确实由本函数初始化成功时调用）。
    fn CoUninitialize();
}

/// `COINIT_APARTMENTTHREADED`
const COINIT_APARTMENTTHREADED: u32 = 0x2;

#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteW(
        hwnd: Handle,
        operation: *const u16,
        file: *const u16,
        parameters: *const u16,
        directory: *const u16,
        show: i32,
    ) -> Handle;
}

/// 把 Rust 字符串转成以 NUL 结尾的 UTF-16。
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 用系统默认程序打开 `path`（目录会在资源管理器里打开）。
///
/// 返回值是**给人看的**错误说明：`ShellExecuteW` 的错误码本身很隐晦
/// （2/3/5/31），直接抛给用户等于没提示。
pub fn open(path: &Path) -> Result<(), String> {
    let file = wide(&path.to_string_lossy());
    // SAFETY: 无参数副作用；返回值仅用于决定是否配对调用 CoUninitialize。
    // S_OK(0) = 本次初始化成功；S_FALSE(1) = 本线程已初始化；RPC_E_CHANGED_MODE
    // = 已以其它模式初始化 —— 后两种都不应调用 CoUninitialize。
    let hr = unsafe { CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED) };
    let own_com = hr == 0;
    let op = wide("open");
    // SAFETY: 三个指针都指向本函数内有效且以 NUL 结尾的 UTF-16 缓冲；
    // hwnd / parameters / directory 传空表示「无父窗口、无参数、用当前目录」。
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            op.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if own_com {
        // SAFETY: 与上面的 CoInitializeEx 配对，仅在本次初始化成功时调用。
        unsafe { CoUninitialize() };
    }
    let code = result as isize;
    if code > SUCCESS_THRESHOLD {
        return Ok(());
    }
    Err(match code {
        0 => "系统资源不足，无法打开".to_string(),
        2 => "找不到文件或目录".to_string(),
        3 => "找不到路径".to_string(),
        5 => "拒绝访问".to_string(),
        31 => "没有可以打开它的关联程序".to_string(),
        other => format!("打开失败，Windows 错误码 {other}"),
    })
}
