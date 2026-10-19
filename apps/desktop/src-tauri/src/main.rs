// Windows 下发布版不弹控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // 诊断入口：`--open-log-dir <路径>` 只调用一次 ShellExecute 然后退出，
    // 退出码 0 = 成功、1 = 失败、2 = 缺参数。
    //
    // 保留它是因为「打开目录」这条路径依赖系统 shell，出问题时无法靠界面
    // 点击来定位；有了这个入口就能用一条命令验证，并从退出码判断结果。
    if args.len() >= 2 && args[1] == "--open-log-dir" {
        let code = match args.get(2) {
            None => 2,
            Some(target) => match fh6_hud_desktop_lib::open_path(std::path::Path::new(target)) {
                Ok(()) => 0,
                Err(_) => 1,
            },
        };
        std::process::exit(code);
    }

    fh6_hud_desktop_lib::run()
}
