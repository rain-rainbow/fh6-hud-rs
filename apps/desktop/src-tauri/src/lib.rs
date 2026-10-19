//! FH6 HUD 配置界面 —— 后端实现。
//!
//! # 设计取舍
//!
//! ## 为什么 GUI 不自己承担 HUD 的渲染
//!
//! HUD 是**逐帧软件光栅化 + 分层窗口**，必须长期以极低占用跑在游戏之上。
//! 把它塞进 WebView 里（用 canvas 绘制）会让 CPU 占用翻几倍，而且 Tauri 的
//! 窗口无法做到 `WS_EX_TRANSPARENT` 点击穿透 + 逐像素 alpha 的分层合成。
//! 因此 GUI **只做配置与进程管理**：改参数 → 写配置文件 → 重启子进程。
//!
//! ## 为什么用「配置文件」作为两个进程的唯一接口
//!
//! `fh6-hud` / `fh6-relay` 与 GUI 共享 `fh6-config` crate 里的 [`AppConfig`]。
//! 好处是：配置格式只有一份定义，GUI 保存后子进程直接读，不需要任何 IPC
//! 协议；用户也可以手改 JSON，GUI 照样能读。
//!
//! ## 托盘与后台常驻
//!
//! 关闭窗口默认**收到托盘**而不是退出（`apps.close_to_tray`）：用户的预期是
//! 「配置一次，后台常驻」，误点关闭不应该把 HUD 一起关掉。真正退出走托盘菜单。

#![deny(unsafe_code)]
#![warn(missing_docs)]
#![warn(clippy::all)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

#[cfg(windows)]
mod single_instance;

use fh6_config::AppConfig;
use serde::Serialize;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, State, WindowEvent};

/// 配置文件默认文件名（与 `fh6_config::app::DEFAULT_FILE_NAME` 一致）。
const CONFIG_NAME: &str = "fh6-hud.config.json";

/// 子进程标识。
const KIND_RELAY: &str = "relay";
const KIND_HUD: &str = "hud";

/// 全局状态。
pub struct AppState {
    /// 当前使用的配置文件路径。
    config_path: Mutex<PathBuf>,
    /// 由本界面启动的子进程。
    children: Mutex<HashMap<String, Child>>,
    /// 关闭窗口时是否收到托盘（由配置同步而来）。
    close_to_tray: AtomicBool,
}

/// 供界面显示的运行状态。
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    /// 配置文件绝对路径。
    pub config_path: String,
    /// relay 是否在运行。
    pub relay_running: bool,
    /// HUD 是否在运行。
    pub hud_running: bool,
    /// 实际解析到的 relay 可执行文件。
    pub relay_exe: String,
    /// 实际解析到的 HUD 可执行文件。
    pub hud_exe: String,
    /// relay 可执行文件是否存在。
    pub relay_exe_ok: bool,
    /// HUD 可执行文件是否存在。
    pub hud_exe_ok: bool,
}

// ---------------------------------------------------------------- 路径解析

/// 在若干候选位置里找可执行文件。
///
/// 查找顺序（先近后远）：
/// 1. 配置里显式写明的路径（若非空）；
/// 2. GUI 自身所在目录；
/// 3. 从 GUI 目录逐级向上的每个祖先目录，以及其下的 `target/release`、`bin`。
///
/// 这样同时覆盖两种部署形态：
/// * **开发目录**：GUI 在 `apps/desktop/src-tauri/target/debug/`，
///   而 HUD 在 `<仓库根>/target/release/` —— 靠「逐级向上」找到；
/// * **打包目录**：三者放在同一文件夹 —— 靠第 2 条命中。
fn resolve_exe(explicit: &str, name: &str) -> PathBuf {
    let leaf = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    let explicit = explicit.trim();
    if !explicit.is_empty() {
        return PathBuf::from(explicit);
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(cur) = std::env::current_exe() {
        let mut dir = cur.parent().map(|p| p.to_path_buf());
        // 最多向上 8 层，避免异常目录结构导致长时间遍历
        let mut depth = 0;
        while let Some(d) = dir {
            candidates.push(d.join(&leaf));
            candidates.push(d.join("target").join("release").join(&leaf));
            candidates.push(d.join("bin").join(&leaf));
            dir = d.parent().map(|p| p.to_path_buf());
            depth += 1;
            if depth > 8 {
                break;
            }
        }
    }
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(&leaf))
}

/// 默认配置文件路径：GUI 可执行文件同目录。
fn default_config_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(CONFIG_NAME)))
        .unwrap_or_else(|| PathBuf::from(CONFIG_NAME))
}

// ---------------------------------------------------------------- 命令

/// 读取当前配置文件路径。
#[tauri::command]
fn config_path(state: State<'_, AppState>) -> String {
    state
        .config_path
        .lock()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// 读取配置。文件不存在时返回默认值（与两个命令行程序的行为一致）。
#[tauri::command]
fn load_config(state: State<'_, AppState>) -> Result<AppConfig, String> {
    let path = state
        .config_path
        .lock()
        .map_err(|_| "状态锁损坏".to_string())?
        .clone();
    let (cfg, _note) = AppConfig::load(&path);
    state
        .close_to_tray
        .store(cfg.apps.close_to_tray, Ordering::Relaxed);
    Ok(cfg)
}

/// 保存配置（原子写入），并同步运行期开关。
#[tauri::command]
fn save_config(state: State<'_, AppState>, mut config: AppConfig) -> Result<(), String> {
    config.sanitize();
    let path = state
        .config_path
        .lock()
        .map_err(|_| "状态锁损坏".to_string())?
        .clone();
    config.save(&path)?;
    state
        .close_to_tray
        .store(config.apps.close_to_tray, Ordering::Relaxed);
    Ok(())
}

/// 设置配置文件的另一个路径（切换配置）。
#[tauri::command]
fn set_config_path(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let p = path.trim();
    if p.is_empty() {
        return Err("路径不能为空".to_string());
    }
    *state.config_path.lock().map_err(|_| "状态锁损坏".to_string())? = PathBuf::from(p);
    Ok(())
}

/// 默认配置路径（供界面「恢复默认位置」用）。
#[tauri::command]
fn default_path() -> String {
    default_config_path().to_string_lossy().to_string()
}

/// 查询运行状态。
#[tauri::command]
fn status(state: State<'_, AppState>, config: AppConfig) -> Status {
    let relay_exe = resolve_exe(&config.apps.relay_exe, "fh6-relay");
    let hud_exe = resolve_exe(&config.apps.hud_exe, "fh6-hud");
    let (relay_running, hud_running) = {
        let mut guard = state.children.lock().unwrap_or_else(|e| e.into_inner());
        (
            alive(&mut guard, KIND_RELAY),
            alive(&mut guard, KIND_HUD),
        )
    };
    Status {
        config_path: state
            .config_path
            .lock()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
        relay_running,
        hud_running,
        relay_exe: relay_exe.to_string_lossy().to_string(),
        hud_exe: hud_exe.to_string_lossy().to_string(),
        relay_exe_ok: relay_exe.is_file(),
        hud_exe_ok: hud_exe.is_file(),
    }
}

/// 检查某类子进程是否还活着；已退出的顺手清理掉。
fn alive(map: &mut HashMap<String, Child>, kind: &str) -> bool {
    if let Some(child) = map.get_mut(kind) {
        match child.try_wait() {
            Ok(None) => return true,
            // 已退出或查询失败：清理，视为未运行
            _ => {
                map.remove(kind);
            }
        }
    }
    false
}

/// 启动指定部分（`relay` / `hud` / `all`）。
#[tauri::command]
fn start(state: State<'_, AppState>, config: AppConfig, which: String) -> Result<String, String> {
    let path = state
        .config_path
        .lock()
        .map_err(|_| "状态锁损坏".to_string())?
        .clone();
    let mut started = Vec::new();
    let mut guard = state.children.lock().map_err(|_| "状态锁损坏".to_string())?;

    let mut spawn = |kind: &str, exe: PathBuf, args: Vec<String>| -> Result<(), String> {
        if alive(&mut guard, kind) {
            return Err(format!("{kind} 已在运行"));
        }
        if !exe.is_file() {
            return Err(format!(
                "找不到可执行文件 {}，请在「应用」里填写正确路径",
                exe.display()
            ));
        }
        let mut cmd = Command::new(&exe);
        cmd.args(&args)
            .current_dir(exe.parent().unwrap_or(Path::new(".")))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            // 不弹出控制台窗口
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let child = cmd
            .spawn()
            .map_err(|e| format!("启动 {} 失败: {e}", exe.display()))?;
        guard.insert(kind.to_string(), child);
        started.push(kind.to_string());
        Ok(())
    };

    let args = vec!["--config".to_string(), path.to_string_lossy().to_string()];
    let want_relay = which == "all" || which == KIND_RELAY;
    let want_hud = which == "all" || which == KIND_HUD;

    let mut errors = Vec::new();
    if want_relay {
        let exe = resolve_exe(&config.apps.relay_exe, "fh6-relay");
        if let Err(e) = spawn(KIND_RELAY, exe, args.clone()) {
            errors.push(e);
        }
    }
    if want_hud {
        let exe = resolve_exe(&config.apps.hud_exe, "fh6-hud");
        if let Err(e) = spawn(KIND_HUD, exe, args) {
            errors.push(e);
        }
    }

    if started.is_empty() && !errors.is_empty() {
        return Err(errors.join("；"));
    }
    let mut msg = format!("已启动: {}", started.join(" + "));
    if !errors.is_empty() {
        msg.push('；');
        msg.push_str(&errors.join("；"));
    }
    Ok(msg)
}

/// 停止指定部分（`relay` / `hud` / `all`）。
#[tauri::command]
fn stop(state: State<'_, AppState>, which: String) -> Result<String, String> {
    let mut guard = state.children.lock().map_err(|_| "状态锁损坏".to_string())?;
    let mut stopped = Vec::new();
    let kinds: Vec<&str> = if which == "all" {
        vec![KIND_RELAY, KIND_HUD]
    } else {
        vec![which.as_str()]
    };
    for kind in kinds {
        if let Some(mut child) = guard.remove(kind) {
            // 先礼后兵：kill 是唯一能保证立即退出的方式（两个程序都不处理 Ctrl+C 消息）
            let _ = child.kill();
            let _ = child.wait();
            stopped.push(kind.to_string());
        }
    }
    if stopped.is_empty() {
        Ok("没有正在运行的进程".to_string())
    } else {
        Ok(format!("已停止: {}", stopped.join(" + ")))
    }
}

/// 在资源管理器里打开日志所在目录。
#[tauri::command]
fn open_log_dir(config: AppConfig) -> Result<(), String> {
    // 基准目录取 **HUD 可执行文件所在目录**，而不是 GUI 自己的目录，也不是
    // 当前工作目录：日志是子进程写的，用子进程的目录才能指对地方。
    let hud_exe = resolve_exe(&config.apps.hud_exe, "fh6-hud");
    let base = hud_exe
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let target = config.logging.resolved_path_from(&base);
    let target = target
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or(target);
    let _ = std::fs::create_dir_all(&target);
    #[cfg(windows)]
    {
        // 用 shell（`start`）打开目录，**不要**直接 `CreateProcess("explorer.exe")`。
        //
        // 实测（本机 Win10 19045）：直接启动 explorer 会以 0xc0000142
        // （STATUS_DLL_INIT_FAILED）退出，并弹出「应用程序无法正常启动」对话框 ——
        // 因为已有 shell 在运行，新起的 explorer 需要走「转交给现有 shell」的路径，
        // 直接创建进程会失败。`start` 内部走 ShellExecute，不会新建 explorer 进程。
        let dir_str = target.to_string_lossy().to_string();
        Command::new("cmd")
            .args(["/c", "start", "", &dir_str])
            .spawn()
            .map_err(|e| format!("打开目录失败: {e}"))?;
    }
    Ok(())
}

// ---------------------------------------------------------------- 托盘

/// 构建托盘图标与菜单。
fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "显示配置窗口", true, None::<&str>)?;
    let start = MenuItem::with_id(app, "start", "启动 HUD + relay", true, None::<&str>)?;
    let stop = MenuItem::with_id(app, "stop", "停止全部", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &start, &stop, &sep, &quit])?;

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("FH6 HUD 配置")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "start" => {
                let _ = tray_start(app);
            }
            "stop" => {
                let _ = tray_stop(app);
            }
            "quit" => {
                stop_all(app);
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

/// 显示主窗口。
fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// 托盘菜单「启动」：用磁盘上的配置启动两个进程。
fn tray_start(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let path = state
        .config_path
        .lock()
        .map_err(|_| "状态锁损坏".to_string())?
        .clone();
    let (cfg, _) = AppConfig::load(&path);
    start(state, cfg, "all".to_string()).map(|_| ())
}

/// 托盘菜单「停止」。
fn tray_stop(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    stop(state, "all".to_string()).map(|_| ())
}

/// 退出前杀掉所有子进程，避免 HUD 残留在屏幕上。
fn stop_all(app: &AppHandle) {
    let state = app.state::<AppState>();
    // 先把 `Result<MutexGuard>` 绑到局部变量：若直接写在 `if let` 里，
    // 这个临时值的析构会晚于 `state`，触发 E0597（借用活得比 owner 长）。
    let locked = state.children.lock();
    if let Ok(mut guard) = locked {
        for (_, mut child) in guard.drain() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

// ---------------------------------------------------------------- 入口

/// 显式指定 WebView2 的用户数据目录，并预先创建它。
///
/// # 为什么必须显式指定
///
/// 实测（本机 Win10 19045 + WebView2 155）：不指定时 `WebviewWindow` 创建失败，
/// 报 `HRESULT 0x8000FFFF（灾难性故障）`，进程在 setup 阶段直接 panic。
/// 但两个候选默认目录（`%APPDATA%` / `%LOCALAPPDATA%` 下的包名）都**确实可写**
/// —— 说明不是权限问题，而是 WebView2 对默认路径的某种拒绝。
///
/// 指定成一个确定存在的目录后**立即正常**。因此这里不猜原因，直接显式给定：
///
/// * 位置选在**可执行文件同目录**的 `webview2-data/`，与配置文件同处一地，
///   整个文件夹拷走即可迁移（与本项目「绿色版」的取向一致）；
/// * 目录不存在时先创建：WebView2 对不存在的路径同样会报错。
///
/// 该环境变量必须在创建 WebView **之前**设置，所以放在 `run()` 的最开头。
fn prepare_webview_data_dir() {
    let dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("webview2-data")))
        .unwrap_or_else(|| PathBuf::from("webview2-data"));
    if std::fs::create_dir_all(&dir).is_ok() {
        std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &dir);
    }
}

/// 启动应用。
pub fn run() {
    // 单实例保护：若已有实例在运行，唤回它的窗口并直接返回。
    //
    // 这对「关闭窗口 = 收到托盘」的模式是必需的：用户看不到托盘图标时会以为
    // 程序已退出，于是再双击一次；没有这层保护就会不断堆积实例。
    #[cfg(windows)]
    if !single_instance::acquire() {
        return;
    }

    prepare_webview_data_dir();

    tauri::Builder::default()
        .manage(AppState {
            config_path: Mutex::new(default_config_path()),
            children: Mutex::new(HashMap::new()),
            close_to_tray: AtomicBool::new(true),
        })
        .invoke_handler(tauri::generate_handler![
            config_path,
            default_path,
            load_config,
            save_config,
            set_config_path,
            status,
            start,
            stop,
            open_log_dir
        ])
        .setup(|app| {
            // 托盘是**可选能力**：某些环境（远程会话、精简系统、Explorer 未就绪）
            // 会拒绝创建托盘图标。此时不应该让整个配置界面打不开 —— 打印告警后继续。
            match build_tray(app.handle()) {
                Ok(()) => {}
                Err(e) => eprintln!("[warn] 托盘图标创建失败（界面仍可用）：{e}"),
            }

            // 首次读取配置：同步 close_to_tray，必要时自动启动子进程
            let state = app.state::<AppState>();
            let path = state
                .config_path
                .lock()
                .map(|p| p.clone())
                .unwrap_or_else(|_| default_config_path());
            let (cfg, _note) = AppConfig::load(&path);
            state
                .close_to_tray
                .store(cfg.apps.close_to_tray, Ordering::Relaxed);
            if cfg.apps.auto_start {
                let _ = start(app.state::<AppState>(), cfg, "all".to_string());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                let to_tray = app
                    .state::<AppState>()
                    .close_to_tray
                    .load(Ordering::Relaxed);
                if to_tray {
                    // 收到托盘而不是退出：这是「后台常驻」的预期行为
                    api.prevent_close();
                    let _ = window.hide();
                } else {
                    stop_all(app);
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("Tauri 应用启动失败");
}
