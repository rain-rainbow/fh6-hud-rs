# fh6-hud-rs

**《极限竞速：地平线6》轻量原生遥测 HUD + UDP 分发器（Rust）**

一个用 Rust 从零实现的 FH6「数据输出」遥测工具集。目标是用**原生编译 + 轻量渲染**
替代 Electron / Python 方案，把内存与 CPU 占用压到最低，同时解决一个实际痛点：
**让 HUD 和飞智八爪鱼5 的自适应扳机能够同时工作**。

> English version: [see below](#english)

---

## 目录

- [项目状态](#项目状态)
- [它解决什么问题](#它解决什么问题)
- [架构](#架构)
- [快速开始](#快速开始)
- [配置](#配置)
- [工程结构](#工程结构)
- [测试](#测试)
- [参考来源与许可声明](#参考来源与许可声明)
- [已知限制](#已知限制)
- [English](#english)

---

## 项目状态

✅ **可运行**。`fh6-relay` 与 `fh6-hud` 都能实际运行。

| 模块 | 状态 | 说明 |
|---|---|---|
| `fh6-core` — 数据包解析 | ✅ 完成 | 324 字节包、88 字段、精确偏移表、派生量计算 |
| `fh6-core` — 漂移状态机 | ✅ 完成 | FLOW / SPIN RISK / 反打 / 风格 / 事件弹窗，纯逻辑零分配 |
| `fh6-config` — 配置模型 | ✅ 完成 | 外观 / 网络端点 / 分发目标 / 日志，三者共享同一份配置 |
| `fh6-relay` — UDP 分发器 | ✅ 完成 | 让 HUD 与飞智自适应扳机同时工作；目标可任意增删 |
| `fh6-hud` — 窗口与光栅化 | ✅ 完成 | 分层透明窗口 + 自实现软件光栅化（见 ADR-011） |
| `fh6-hud` — 仪表绘制 | ✅ 完成 | 主角度表 / G 表 / 输入 / 滑移 / 状态面板 / 事件弹窗 |
| `fh6-hud` — 中文界面 | ✅ 完成 | 默认中文，可切英文；行距按字形自动适配（见 ADR-013） |
| `fh6-hud` — 背景透明化 | ✅ 完成 | 面板 / G 表 / 弹窗底衬均可调到 0（完全透明） |
| `apps/desktop` — 图形化配置 | ✅ 完成 | Tauri 2，常驻托盘，启停两个进程、编辑配置 |

**当前可用**：relay 分发 + HUD 实时显示 + 图形化配置界面。
`--selftest` 可离屏渲染色样图用于核对，`--capture` 可抓取真实绘制帧。

---

## 它解决什么问题

### 问题：FH6 只支持**一个**遥测目标，而飞智霸占了 5300

FH6 官方文档明确：

> `A single packet format is sent to the configured address`

只能填**一个** IP + **一个**端口。而飞智空间站的八爪鱼5 自适应扳机模块
（`mods/forza_trigger/ForzaDualSense.exe`）会**独占监听 `127.0.0.1:5300`**，
且其配置文件里**没有端口项**，用户改不了。

想同时跑 HUD 就会二选一：要么没 HUD，要么没自适应扳机。

### 为什么不能靠 `SO_REUSEADDR` 共享端口

这是常见误解。**本机实测（Windows 10 19045）**：

| 场景 | 结果 |
|---|---|
| 两个套接字都设 `SO_REUSEADDR`，绑同一端口 | 都能 bind 成功，但**单播包只投递给第一个绑定者** |
| 一个设、一个不设 | 不设的那个 **bind 直接失败** |
| 都不设 | 第二个 bind 失败 |

Windows 上 UDP 单播包**不会复制**给多个绑定者。**必须显式转发。**

### 解法：让游戏发给 relay，relay 再扇出

关键突破口：**游戏里的「数据输出IP地址」和「数据输出IP端口」都是可以自由
填写的** —— 5300 只是飞智推荐的取值，不是游戏强制的。

所以**不去抢 5300**（抢了会把飞智顶掉），而是让游戏改发到一个新端口：

```
FH6  Data Out ──►  127.0.0.1:5301   (fh6-relay 独占监听)
                         │
           ┌─────────────┴─────────────┐
           ▼                           ▼
    127.0.0.1:5300                127.0.0.1:5302
    飞智 ForzaDualSense              fh6-hud
    （自适应扳机照常工作）            （本项目 HUD）
```

**你只需要在游戏里改一个数字**（端口 5300 → 5301），其余全自动，
飞智完全感知不到变化。

**顺带好处**：天然支持任意多个消费者 —— 想再接 SimHub、想录数据脚本，
`--to 127.0.0.1:8001` 加一个目标即可。

---

## 架构

```
┌─────────────────────────────────────────────────────────┐
│  crates/fh6-core        纯逻辑库（无 I/O、无 GUI）        │
│  ├── pack.rs      88 个字段的字节偏移常量 + 来源可信度标注 │
│  ├── raw.rs       #[repr(C)] 结构体，作布局交叉验证        │
│  ├── telemetry.rs 解析器（显式小端，全 crate forbid unsafe）│
│  ├── derived.rs   派生量：km/h、漂移角、G 值、输入百分比    │
│  └── drift.rs     漂移状态机：FLOW / RISK / 风格 / 事件    │
└─────────────────────────────────────────────────────────┘
              ▲                              ▲
              │                              │
┌─────────────┴───────────┐    ┌─────────────┴──────────────────────┐
│  crates/fh6-relay       │    │  crates/fh6-hud                    │
│  UDP 分发器（lib + bin） │    │  ├─ gauge.rs    纯几何/配色（可测）│
│  5301 → 5300 + 5302     │    │  ├─ raster.rs   软件光栅化（ADR-011）│
│  逐字节透明转发          │    │  ├─ textmask.rs 文字 alpha 掩码     │
└─────────────────────────┘    │  ├─ window.rs   分层透明窗口        │
                               │  ├─ settings.rs 可调参数模型        │
                               │  ├─ shared.rs   收包↔渲染快照交换   │
                               │  └─ timing.rs   帧率与活动检测      │
                               └────────────────────────────────────┘
```

设计理由（含实测数据）见 [`docs/架构决策.md`](docs/架构决策.md)，
数据包格式速查表见 [`docs/协议速查.md`](docs/协议速查.md)。

---

## 快速开始

### 构建

需要 Rust 工具链。Windows 上推荐 GNU target（可复用 MinGW，无需装 MSVC）：

```bash
# 若尚未安装 Rust
rustup default stable-x86_64-pc-windows-gnu

git clone git@github.com:rain-rainbow/fh6-hud-rs.git
cd fh6-hud-rs
cargo build --release
```

产物：

```
target/release/fh6-relay.exe
target/release/fh6-hud.exe
```

### 使用 relay

1. 启动 relay：

   ```bash
   fh6-relay
   ```

   默认监听 `127.0.0.1:5301`，转发给 `127.0.0.1:5300`（飞智）
   与 `127.0.0.1:5302`（HUD）。

2. **在游戏里改设置** —— `设置 > 抬头显示器与游戏`：

   | 设置项 | 值 |
   |---|---|
   | 数据输出 | 开启 |
   | 数据输出IP地址 | `127.0.0.1` |
   | 数据输出IP端口 | **`5301`** ← 注意不是 5300 |

3. 启动飞智空间站（自适应扳机照常工作）。

### 常用参数

```bash
# 追加一个转发目标（例如 SimHub）
fh6-relay --to 127.0.0.1:8001

# 换个监听端口
fh6-relay --listen 127.0.0.1:5400

# 调试：打印每个包
fh6-relay --verbose

# 查看帮助
fh6-relay --help
```

> **为什么默认用 5301**：官方 FH6 文档提醒
> `Avoid ports 5200 through 5300, as the game binds its own outgoing socket
> to a port in this range.` —— 游戏自身会占用 5200–5300，
> 5301 刚好在其外。

---

## 配置

三个程序（`fh6-hud`、`fh6-relay`、图形界面）共用**同一份配置文件**
`fh6-hud.config.json`，默认放在可执行文件同目录。

```bash
# HUD：读配置 + 用配置里的监听端点
fh6-hud --config fh6-hud.config.json

# relay：读同一份配置（监听端点、分发目标都在里面）
fh6-relay --config fh6-hud.config.json

# 紧急情况下也可以只用命令行，不碰配置文件
fh6-relay --listen 127.0.0.1:5401 --to 127.0.0.1:5302
```

### 配置文件长这样

```json
{
  "version": 1,
  "hud": {
    "language": "zh",
    "network": { "listen_ip": "127.0.0.1", "listen_port": 5302 },
    "global_scale": 1.0,
    "input_panel":  { "x": 40, "y": 40,  "panel_opacity": 0.10 },
    "g_meter":      { "backdrop_alpha": 0.12 },
    "popup":        { "backdrop_alpha": 0.22 }
  },
  "relay": {
    "listen_ip": "127.0.0.1",
    "listen_port": 5301,
    "targets": [
      { "enabled": true, "host": "127.0.0.1", "port": 5300, "label": "飞智自适应扳机" },
      { "enabled": true, "host": "127.0.0.1", "port": 5302, "label": "HUD" }
    ]
  },
  "logging": { "enabled": false, "path": "logs/fh6.log", "level": "info" },
  "apps": { "auto_start": false, "close_to_tray": true }
}
```

### 图形化配置界面

```bash
cd apps/desktop/src-tauri
cargo build --release
# 产物：target/release/fh6-hud-desktop.exe
```

界面提供：语言切换、全部透明度滑杆、两个监听端点、**分发目标的增删改**、
日志开关、以及「启动 / 停止」两个进程。关闭窗口默认收到**系统托盘**常驻，
真正退出走托盘菜单。

> 边界情况都做了处理：配置文件损坏 → 回退默认值并提示；旧版本（只有外观参数）
> 的配置 → **自动迁移**到新格式；分发目标与 relay 监听端点重合 → 自动禁用，
> 避免 relay 转发给自己造成包风暴。

#### 构建时的两个坑（都已在代码里解决）

1. **`crate-type` 必须是纯 `rlib`**
   Tauri 官方模板写的是 `["staticlib", "cdylib", "rlib"]`，那是为**移动端**准备的。
   在 Windows + GNU 工具链下，`cdylib` 会把所有符号写进 `list.def`，链接时报
   `error: export ordinal too large: 94449`（PE 导出序号上限 65535）。

2. **必须显式指定 WebView2 的用户数据目录**
   不指定时，实测本机会在创建 WebView 时失败
   （`HRESULT 0x8000FFFF` 或 `os error 5 拒绝访问`），进程直接 panic。
   程序会在**可执行文件同目录**建 `webview2-data/` 并通过
   `WEBVIEW2_USER_DATA_FOLDER` 指过去，之后稳定可用。

   > 若遇到「界面打不开」：先删掉 `webview2-data/` 再启动。
   > 反复启动/强杀时，残留的 WebView2 浏览器进程可能短暂占着该目录，
   > 表现为间歇性失败 —— 等几秒或删目录即可，不是程序缺陷。

#### 首次构建很慢

Tauri 的依赖树有 500+ 个 crate。国内网络下 crates.io 的 CDN 连接容易卡住
（表现为 cargo 长时间无输出、`Established` 连接却无数据）。可用镜像：

```bash
cargo build --release \
  --config 'source.crates-io.replace-with="rsproxy"' \
  --config 'source.rsproxy.registry="sparse+https://rsproxy.cn/index/"'
```

---

## 工程结构

```
fh6-hud-rs/
├── Cargo.toml                  # workspace 根（三个 crate）
├── LICENSE                     # MIT
├── README.md                   # 本文件（中英双文）
├── docs/
│   ├── 架构决策.md              # ADR：为什么这样设计 + 实测依据
│   ├── 协议速查.md              # 88 字段偏移表 + 可信度分级
│   └── 配置文件说明.md          # 配置字段逐项说明
├── crates/
│   ├── fh6-config/             # 配置模型（HUD / relay / 日志 / 应用路径）
│   │   └── src/{lib,settings,relay,logging,app,apps}.rs
│   ├── fh6-core/               # 纯逻辑库
│   │   ├── src/{lib,pack,raw,telemetry,derived,drift}.rs
│   │   └── tests/{derived,drift}_tests.rs
│   ├── fh6-relay/              # UDP 分发器
│   │   ├── src/{lib,main}.rs
│   │   └── tests/relay_tests.rs
│   └── fh6-hud/                # HUD
│       └── src/
│           ├── gauge.rs        # 纯几何/配色（可测）
│           ├── i18n.rs         # 中英文本 + 字形行距系数
│           ├── raster.rs       # 软件光栅化
│           ├── window.rs       # 分层透明窗口
│           └── ...
├── apps/
│   └── desktop/                # 图形化配置界面（Tauri 2，独立 workspace）
│       ├── src/{index.html,main.js,style.css}
│       └── src-tauri/{Cargo.toml,tauri.conf.json,src/lib.rs}
└── tools/
    └── telemetry-probe.ps1     # 遥测链路探针（被动监听，不转发）
```

---

## 测试

```bash
cargo test --workspace
```

**266 个测试，全部不需要启动窗口或连接游戏**，整轮约 1 秒。

| 测试组 | 数量 | 覆盖内容 |
|---|---|---|
| `fh6-config` 单元测试 | 65 | 参数夹紧与默认值、序列化往返、**旧版配置自动迁移**、日志轮转与时间戳格式、**自转发目标拦截**、原子保存、损坏配置回退 |
| `fh6-core` 单元测试 | 6 | 结构体大小 = 324、偏移量互不重叠、短包报错、超长包忽略尾部 |
| `fh6-core` 派生量测试 | 21 | 单位换算、漂移角、G 值符号与钳制、输入百分比、NaN 兜底、编解码往返 |
| `fh6-core` 漂移状态机测试 | 36 | 反打量公式、保持计时防抖、FLOW/风险标签边界、状态机优先级、弹窗优先级与抗重复续期、SPIN SAVE 武装→恢复→冷却时序、GOD/CAT 彩蛋、帧率无关性 |
| `fh6-core` 文档测试 | 3 | 公开 API 的使用示例 |
| `fh6-hud` 库测试 | 104 | 仪表几何（角度→坐标、弧线采样、G 映射、脏区）、**中英文本穷尽性**（中文模式不得残留英文名词）、排版行距系数、软件光栅化图元、时序与帧率节流 |
| `fh6-hud` 二进制测试 | 15 | 端口解析、参数解析、默认端点 |
| `fh6-relay` 单元测试 | 8 | 默认端口、参数解析、等号写法、配置文件参数、错误处理 |
| `fh6-relay` 端到端测试 | 8 | **真实 UDP 收发**：多目标扇出、逐字节透明、多包、目标不可达容错、短包、端口占用报错 |

### 开发中被测试抓到的缺陷

以下都是**测试先于修复发现**的，记录下来作为「为什么值得写测试」的实例：

1. **横向 G 符号反了**。初版公式写成 `az*fx - ax*fz`，测试报出
   「右转得到 -1G」。定案为 `ax*fz - az*fx`（右转为正，ISO 8855 约定），
   详见 [`docs/架构决策.md`](docs/架构决策.md) 的 ADR-010。
2. **323 vs 324 的 1 字节差**。`size_of::<RawPacket>()` 实测为 **324** 而非
   323 —— 差异全是**结构体尾部对齐填充**（含 `f32`/`i32`，对齐要求 4）。
   这正好解释了官方那句没人能解释的 `Total packet size: 324 bytes.`：
   **第 324 字节是编译器的对齐填充，不是字段。**
3. **弹窗被无限续期**。状态机初版缺少「上一帧状态不同」守卫，
   只要风格持续为 `LOCKED` 就会**每帧重新请求弹窗**，导致弹窗永不消失。
   参考实现用 `last_style_label != "LOCKED"` 防重，此处等价实现。
4. **测试脚手架自身的竞态**。relay 端到端测试初版先 `bind(0)` 取端口号、
   再 `drop` 让 relay 绑同一个号，中间存在窗口被别人抢走，导致 relay 在线程里
   静默 panic、表现为「5 个用例一起失败」。产品代码从头到尾都是对的 ——
   修好脚手架（先占住再交接 + 轮询确认已绑定）后全部通过。
5. **测试断言写错，而非代码错**。反打量测试初版断言 `> 50`，但按公式
   30° / 60% 打方向的理论值是 44.17。诊断表逐角度核对后确认实现正确，
   改成按公式断言。

---

## 参考来源与许可声明

本项目是 **clean-room 独立实现**：协议格式依据**官方文档**与公开的社区逆向资料，
**未复制任何第三方项目的源代码**。

### 官方文档

- [Forza Horizon 6 — Data Out Documentation](https://support.forza.net/hc/en-us/articles/51744149102611-Forza-Horizon-6-Data-Out-Documentation)
  —— 包长、字段类型与顺序、游戏内设置项、回环支持。
  直连可能返回 403；可改用 Zendesk API：
  `https://support.forza.net/api/v2/help_center/en-us/articles/51744149102611.json`
- [Forza Motorsport — Data Out Documentation](https://support.forza.net/hc/en-us/articles/21742934024211)
  —— 「Sled / Dash」格式定义与 60 packets/second 的出处。
  ⚠️ 注意：FH6 **没有**这些格式选项，借用其术语时需说明。

### 用于交叉验证偏移量的社区实现（未复制代码）

- [benweier/forza-telemetry](https://github.com/benweier/forza-telemetry)
- [richstokes/Forza-data-tools](https://github.com/richstokes/Forza-data-tools)
- [0x20F/forza-telemetry](https://github.com/0x20F/forza-telemetry)

### 功能设计参考

- [O93-3/093_Drift_HUD](https://github.com/O93-3/093_Drift_HUD)
  —— 本项目在**功能范围与交互设计**上参考了这个 Python/PyQt6 的 FH6 漂移 HUD。

  ⚠️ **重要法律说明**：该仓库**没有 LICENSE 文件**，只有一份 260 字节的
  `LICENSE_NOTICE.txt` 免责声明（GitHub 标记 `spdx_id: NOASSERTION`）。
  无授权即默认**保留所有权利**，因此：

  * ❌ 本项目**没有**复制、翻译或改写其任何代码；
  * ❌ 其 `cars.json`（638 条车名映射）版权属原作者，**未被使用**；
  * ✅ 仅参考了其**公开可观察的行为与视觉规格**（例如漂移角取
    `atan2(VelocityX, VelocityZ)`、后轴滑移取 `TireCombinedSlip` 绝对值的平均），
    这些属于可从公开协议文档推导的范畴。

### 硬件相关的第三方信息

- 飞智（Flydigi）八爪鱼5 / 空间站软件的 5300 端口行为，来自**本机实测**
  （`netstat` 观察进程绑定 + 二进制字符串分析），非官方公开文档。

### 本项目许可

[MIT](LICENSE) © rain-rainbow

---

## 已知限制

| 限制 | 说明 |
|---|---|
| **渲染层未实现** | 目前 `fh6-hud` 只有命令行骨架，还不能显示 HUD。 |
| **偏移量未经真机逐字节核对** | 偏移表源自官方类型顺序 + 社区逆向交叉验证。待用真实抓包核对，见 `docs/协议速查.md`。 |
| **仅 Windows** | 依赖 Windows 的窗口与端口语义。协议解析层（`fh6-core`）本身是跨平台的。 |
| **发送频率不是固定 60 Hz** | 官方只说「等于游戏帧率」，高刷环境下会更高。解析端**不应假定固定包率**。 |
| **车名表未实现** | 官方不提供 `CarOrdinal → 车名` 映射。需要自建，不能使用参考实现的数据。 |
| **漂移评分阈值未实现** | 该逻辑没有公开规范，需逆向 + 实车对拍。 |

---

# English

## fh6-hud-rs

**A lightweight native telemetry HUD and UDP fan-out relay for Forza Horizon 6 (Rust).**

A from-scratch Rust implementation of FH6 "Data Out" telemetry tooling. The goal is to
replace Electron / Python stacks with native compilation and lightweight rendering to
minimize memory and CPU usage — and to solve a real-world annoyance:
**making a HUD and the Flydigi Vader 5 adaptive triggers work at the same time.**

> 中文版见[上方](#fh6-hud-rs)

### Status

✅ **Runnable.** Relay fan-out, the HUD renderer, and a GUI configurator all work.

| Component | Status | Notes |
|---|---|---|
| `fh6-core` — packet parsing | ✅ Done | 324-byte packet, 88 fields, exact offset table, derived values |
| `fh6-core` — drift state machine | ✅ Done | FLOW / SPIN RISK / counter-steer / style / events; pure logic, zero allocation |
| `fh6-config` — configuration model | ✅ Done | Appearance, endpoints, fan-out targets, logging — one file shared by all three programs |
| `fh6-relay` — UDP fan-out | ✅ Done | Arbitrary number of targets, each independently enabled |
| `fh6-hud` — window & rendering | ✅ Done | Layered transparent window + hand-written software rasterizer (ADR-011) |
| `fh6-hud` — gauge drawing | ✅ Done | Angle gauge / G meter / input / slip / status panels / event popups |
| `fh6-hud` — Chinese UI | ✅ Done | Chinese by default, switchable to English; line height adapts per script (ADR-013) |
| `fh6-hud` — transparent background | ✅ Done | Panel / G-ring / popup backdrops can all be dialed to 0 |
| `apps/desktop` — GUI configurator | ✅ Done | Tauri 2, lives in the system tray, starts/stops both processes |

**Verified**: 266 tests pass; `cargo fmt --check` and `cargo clippy -D warnings` are clean.

### The problem it solves

FH6 sends its telemetry to **exactly one** IP and port:

> `A single packet format is sent to the configured address`

Meanwhile, Flydigi Space Station's adaptive-trigger module for the Vader 5
(`mods/forza_trigger/ForzaDualSense.exe`) **exclusively binds `127.0.0.1:5300`**
and exposes **no port setting**. So you had to choose: HUD *or* adaptive triggers.

#### Why `SO_REUSEADDR` does not help

A common misconception. **Measured on Windows 10 19045**:

| Scenario | Result |
|---|---|
| Both sockets set `SO_REUSEADDR`, same port | Both bind successfully, but **unicast datagrams go only to the first binder** |
| One sets it, the other does not | The second `bind` **fails outright** |
| Neither sets it | Second `bind` fails |

Windows does **not** duplicate UDP unicast datagrams across binders. **Explicit
forwarding is required.**

#### The solution

The key insight: **the game's "Data Out IP" and "Data Out IP Port" are
freely configurable** — `5300` is merely what Flydigi recommends, not a game
requirement. So instead of racing Flydigi for port 5300 (which would starve it),
point the game at a new port and fan out from there:

```
FH6  Data Out ──►  127.0.0.1:5301   (fh6-relay exclusively)
                         │
           ┌─────────────┴─────────────┐
           ▼                           ▼
    127.0.0.1:5300                127.0.0.1:5302
    Flydigi ForzaDualSense           fh6-hud
    (adaptive triggers intact)      (this project's HUD)
```

**You change one number in-game** (port `5300` → `5301`). Flydigi never notices.

Bonus: this naturally supports any number of consumers — add
`--to 127.0.0.1:8001` for SimHub, a data logger, whatever.

### Quick start

```bash
rustup default stable-x86_64-pc-windows-gnu   # avoids a ~5 GB MSVC install
git clone git@github.com:rain-rainbow/fh6-hud-rs.git
cd fh6-hud-rs
cargo build --release
```

Run the relay:

```bash
fh6-relay              # listens on 127.0.0.1:5301, fans out to 5300 and 5302
fh6-relay --help
```

Then in-game, under **Settings > HUD and Gameplay**:

| Setting | Value |
|---|---|
| Data Out | On |
| Data Out IP Address | `127.0.0.1` |
| Data Out IP Port | **`5301`** ← not 5300 |

### Documentation

- [`docs/协议速查.md`](docs/协议速查.md) — full 88-field offset table with
  per-offset confidence levels (official vs. reverse-engineered).
- [`docs/架构决策.md`](docs/架构决策.md) — ADRs: every design decision with its
  measured evidence.

### Tests

```bash
cargo test --workspace
```

**84 tests, none of which need a GUI or a running game**, completing in under a second.

Five real defects were caught by tests during development:

1. **Lateral G sign was inverted.** The first formula was `az*fx - ax*fz`; a test
   reported "right turn yields -1G". Settled as `ax*fz - az*fx` (right turn
   positive, per ISO 8855). See ADR-010.
2. **The 323 vs. 324 byte discrepancy.** `size_of::<RawPacket>()` measured exactly
   **324**, not 323 — the extra byte is **struct tail padding** (the struct contains
   `f32`/`i32`, requiring 4-byte alignment). This explains the official
   `Total packet size: 324 bytes.`: **byte 324 is compiler padding, not a field.**
3. **Popups were renewed forever.** The first state machine lacked a
   "previous state differs" guard, so whenever the style stayed `LOCKED` it
   re-requested the popup **every single frame**, keeping it on screen indefinitely.
4. **A race in the test harness itself.** The relay e2e tests originally did
   `bind(0)` to get a port, then `drop`ped it so the relay could bind the same
   number — leaving a window for another process to steal it. The relay then
   panicked silently inside its thread, showing up as "5 tests fail at once".
   The production code was correct from the start.
5. **A wrong assertion, not wrong code.** The counter-steer test asserted `> 50`,
   but the formula's value at 30° with 60% steering is 44.17. Per-angle
   verification confirmed the implementation; the assertion was corrected to
   match the formula.

### Credits and licensing

This is a **clean-room implementation**. The protocol format is derived from
**official documentation** and publicly available community reverse-engineering;
**no third-party source code was copied.**

- Official:
  [FH6 Data Out](https://support.forza.net/hc/en-us/articles/51744149102611-Forza-Horizon-6-Data-Out-Documentation) ·
  [Forza Motorsport Data Out](https://support.forza.net/hc/en-us/articles/21742934024211)
  (source of the "Sled/Dash" terms and the 60 pps figure — note FH6 has **no** packet
  format option, unlike Forza Motorsport)
- Cross-checked against (no code copied):
  [benweier/forza-telemetry](https://github.com/benweier/forza-telemetry) ·
  [richstokes/Forza-data-tools](https://github.com/richstokes/Forza-data-tools) ·
  [0x20F/forza-telemetry](https://github.com/0x20F/forza-telemetry)
- Design reference: [O93-3/093_Drift_HUD](https://github.com/O93-3/093_Drift_HUD)

  ⚠️ **That repository has no LICENSE file** — only a 260-byte
  `LICENSE_NOTICE.txt` disclaimer (GitHub reports `spdx_id: NOASSERTION`).
  Absent a grant, all rights are reserved. Therefore this project
  **does not copy, translate, or adapt any of its code**, and its
  `cars.json` (638 car-name mappings) is **not used**. Only publicly
  observable behaviour and visual specification were referenced.

- Flydigi's port-5300 behaviour was determined by **local measurement**
  (`netstat` observation plus binary string analysis), not from published docs.

**License: [MIT](LICENSE) © rain-rainbow**

### Known limitations

| Limitation | Notes |
|---|---|
| **No renderer yet** | `fh6-hud` is a CLI skeleton; it does not display anything. |
| **Offsets not byte-verified in-game** | The table comes from official field ordering plus community cross-validation. Ground-truth capture is still pending. |
| **Windows only** | Depends on Windows window and port semantics. `fh6-core` itself is portable. |
| **Rate is not a fixed 60 Hz** | The official docs only say "equal to the game's frame rate". Do not assume a fixed rate. |
| **No car-name table** | The game does not publish `CarOrdinal → name`. Must be built from public sources; the reference project's data cannot be reused. |
| **No drift scoring** | That logic has no public specification and would require reverse-engineering plus on-track validation. |
