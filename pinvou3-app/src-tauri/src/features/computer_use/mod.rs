//! Computer Use：模型经单一 `computer_use` 工具观察并操作用户桌面。
//!
//! 模块结构：
//! - `types` —— 动作枚举、按键和弦解析、错误与后端结果结构、坐标空间契约
//! - `scaling` —— 截图缩放（长边 ≤1440）与 截图↔设备↔输入 三层坐标映射
//! - `backend` —— 后端 trait + 每会话一条专用 worker 线程（xcap/enigo 线程亲和）
//! - `guard` —— 同意门控：设置开关、会话授权、速率/预算、停止旗标、T3 确认
//! - `audit` —— append-only JSONL 审计（文本只记长度+SHA-256，永不记明文）
//! - `tool` —— `ComputerUseTool` ToolSpec 实现
//! - `platform` —— 三平台后端与权限引导（`request_permissions`）
//!
//! 集成（composition root：lib.rs 装配 / app::commands 命令面）：
//! - 工厂按会话构造 `ComputerUseTool::new(app_handle, session_id, shared.clone())`，
//!   仅当 `ComputerUseShared::is_enabled()` 为真（设置开关关闭时模型看不到 schema）；
//! - `ComputerUseShared` 全局单例（Arc，经 `.manage()` 共享），设置开关接
//!   `set_enabled`，授权/吊销/急停接 `grant_session` / `revoke_session` / `stop_all`，
//!   T3 确认命令接 `mint_confirmation`；
//! - 前端监听 `computer_use:grant_required` 与 `computer_use:confirm_required`。

mod audit;
mod backend;
mod guard;
mod platform;
mod scaling;
mod tool;
mod types;

// ---- 工具与共享状态（tool / guard）：composition root 与 Tauri 命令消费 ----
pub use self::guard::ComputerUseShared;
pub use self::tool::ComputerUseTool;

// ---- 工具名（types）：ToolPolicy 动态禁用与命令面共用 ----
pub use self::types::TOOL_NAME;

// ---- 平台能力入口（platform）：Tauri 命令消费 ----
pub(crate) use self::platform::{backend_supported, request_permissions};
