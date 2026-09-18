pub(crate) mod geometry;
pub(crate) mod pet_window;
mod platform;
pub(crate) mod selected_pet;

/// 撕离窗口路径入口。实现全部在 platform 适配层；原先的 detach.rs 纯转发
/// 微文件已并入此处（保持 `features::pet::detach::*` 路径不变）。
pub(crate) mod detach {
    pub use super::platform::detach::{begin_detach_drag, point_in_rect};
}
