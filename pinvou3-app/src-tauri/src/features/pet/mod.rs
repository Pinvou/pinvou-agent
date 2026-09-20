pub(crate) mod geometry;
pub(crate) mod pet_window;
mod platform;
pub(crate) mod selected_pet;

// 撕离窗口的两个入口。实现全部在 platform 适配层；原先的 detach.rs 纯转发
// 微文件及其模块壳均已移除，直接在 pet 根 re-export。
pub(crate) use platform::detach::{begin_detach_drag, point_in_rect};
