//! 构建脚本：声明 Win32 导入库。
//!
//! # 为什么需要这个
//!
//! 本项目用手写 `extern "system"` 声明 Win32 函数（而不是引入 `windows` crate，
//! 理由见 `src/win32.rs` 的模块文档）。手写声明的代价是：**链接器不知道这些
//! 符号来自哪个导入库**，必须在构建时显式告知。
//!
//! 需要的两个库：
//!
//! | 库 | 提供 |
//! |---|---|
//! | `gdi32` | `CreateDIBSection`、`CreatePen`、`BitBlt`、`UpdateLayeredWindow` 等绘制与分层窗口接口 |
//! | `user32` | `CreateWindowExW`、`RegisterClassExW`、`PeekMessageW`、`CreateFontW` 等窗口与文本接口 |
//!
//! 注意 `UpdateLayeredWindow` 与 `CreateFontW` 在 **user32**，
//! 而 `CreateDIBSection`、`CreatePen` 在 **gdi32** —— 两者不能互换，
//! 少了任何一个都会在链接期报 `undefined reference`。

fn main() {
    // 只在 Windows 目标上声明；其它平台编译本 crate 时这些符号根本不会被引用
    // （`win32` 模块用 `#[cfg(windows)]` 保护）。
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        println!("cargo:rustc-link-lib=dylib=gdi32");
        println!("cargo:rustc-link-lib=dylib=user32");
    }

    // 源码或构建脚本变化时重新运行
    println!("cargo:rerun-if-changed=build.rs");
}
