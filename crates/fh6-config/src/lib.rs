//! # fh6-config — FH6 HUD 的配置模型
//!
//! 本 crate 只做**配置数据的定义、校验、读写**，不含任何 I/O 线程、渲染或
//! 网络逻辑。把它独立出来的原因有三：
//!
//! 1. **多个二进制共享**：`fh6-hud`（外观 + 监听端口）、`fh6-relay`（分发目标）、
//!    以及 Tauri 配置界面都要读写同一份配置，放在任何一侧都会造成反向依赖；
//! 2. **可完整单元测试**：配置的默认值、夹紧、容错回退都是纯逻辑；
//! 3. **界面与运行时解耦**：GUI 只依赖本 crate，不必把渲染层也拖进去。
//!
//! # 模块划分
//!
//! | 模块 | 内容 |
//! |---|---|
//! | [`settings`] | HUD 外观参数（几何、颜色强度、透明度、语言、监听端点） |
//! | [`relay`] | relay 的监听地址与**可增删的分发目标列表** |
//! | [`logging`] | 可选日志文件的级别、路径、轮转策略 |
//! | [`app`] | 把上面三者组装成的顶层配置文件 `AppConfig` |
//!
//! # 设计原则
//!
//! * **任何外部来源的配置都必须先 `sanitize()`**：渲染与网络层因此可以假定
//!   参数合法，不必到处写防御代码；
//! * **绝不因为配置损坏而让程序起不来**：解析失败回退默认值并返回提示文本；
//! * **向后兼容**：所有结构体都 `#[serde(default)]`，缺字段取默认值，
//!   未知字段忽略。
//!
//! # 法律边界
//!
//! 参考实现 [O93-3/093_Drift_HUD](https://github.com/O93-3/093_Drift_HUD)
//! **没有 LICENSE**（GitHub 标记 `NOASSERTION`）。本 crate 的设计与数值均为
//! 本项目原创，未复制或翻译其任何代码，未使用其数据文件。

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(clippy::all)]

pub mod app;
pub mod apps;
pub mod logging;
pub mod relay;
pub mod settings;

pub use app::{AppConfig, APP_CONFIG_VERSION};
pub use apps::AppsConfig;
pub use logging::{LogConfig, LogLevel, Logger};
pub use relay::{RelayConfig, RelayTarget};
pub use settings::{Language, NetworkConfig, Settings, SETTINGS_VERSION};
