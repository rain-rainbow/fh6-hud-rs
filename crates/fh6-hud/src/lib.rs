//! # fh6-hud — FH6 轻量原生 HUD 叠加层
//!
//! 本 crate 分为两层：
//!
//! | 层 | 可否单元测试 | 内容 |
//! |---|---|---|
//! | [`gauge`] 几何与主题 | ✅ 可以 | 角度→坐标、弧线折线、G 点映射、轨迹环缓冲、脏区累积 |
//! | 渲染后端（待实现） | ❌ 只能肉眼核对 | 描边、辉光、网点、扫描线、字体排版 |
//!
//! 把可测的一半拆出来，是本项目相对参考实现的核心改进之一：参考实现把
//! 渲染、业务、遥测全部耦合在一个 5115 行的 PyQt 类里（125 个方法共享
//! 60 多个可变状态字段），导致**业务逻辑完全无法测试**。
//!
//! # 关键设计要点
//!
//! 1. **窗口必须点击穿透**：加 `WS_EX_TRANSPARENT`。参考实现没有做，
//!    而且把窗口设为整屏，会吃掉鼠标事件挡住游戏操作。
//! 2. **收包与渲染分线程**：UDP 接收独立线程，渲染只读最新快照。
//!    参考实现全挤在 Qt 主线程，掉帧时会丢遥测包（UDP 无重传）。
//! 3. **只在有变化时重绘**：用 [`gauge::DirtyRegion`] 累积脏区。
//!    参考实现每帧无条件全屏重绘，静止时也照样 62.5 fps。
//! 4. **零堆分配热路径**：G 值轨迹用 [`gauge::TrailBuf`] 定长环缓冲，
//!    替代参考实现的 `Vec` 切片重建。
//!
//! # 法律边界
//!
//! 参考实现 [O93-3/093_Drift_HUD](https://github.com/O93-3/093_Drift_HUD)
//! **没有 LICENSE**（GitHub 标记 `NOASSERTION`），默认保留所有权利。
//! 因此本项目：
//!
//! * ✅ 仅参考其**功能与视觉规格**（数值阈值、颜色值、几何比例等
//!   功能性/事实性参数）；
//! * ❌ **未复制或翻译**其任何绘制代码（`draw_*` 等方法）；
//! * ❌ **未使用**其 `cars.json` 等数据文件。
//!
//! 详见 [`docs/架构决策.md`](https://github.com/rain-rainbow/fh6-hud-rs/blob/main/docs/架构决策.md)。

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(clippy::all)]

pub mod gauge;
pub mod settings;

pub use gauge::{Bgra, DirtyRegion, Rect, Theme, TrailBuf};
pub use settings::{Settings, SETTINGS_VERSION};
