use super::prelude::*;
use crate::features::pet::{pet_window as pet_domain, selected_pet as selected_pet_domain};
use pet_domain::*;
use selected_pet_domain::*;

/// 撕离拖拽入口：feature 侧为裸 fn（pet::detach 转发壳已扁平化），不适用
/// domain 模块的 passthrough 宏，直接薄转发。
#[tauri::command]
pub async fn begin_detach_drag(
    kind: String,
    id: Option<String>,
    app: AppHandle,
) -> Result<(), String> {
    crate::features::pet::begin_detach_drag(kind, id, app).await
}

async_command_passthrough!(pet_domain, set_pet_enabled(enabled: bool, app: AppHandle) -> Result<(), String>);
async_command_passthrough!(pet_domain, set_pet_scale(scale: f64, vertical_alignment: Option<String>, activity_visible: Option<bool>, activity_height: Option<f64>, app: AppHandle) -> Result<f64, String>);
async_command_passthrough!(pet_domain, set_pet_activity_visible(visible: bool, activity_height: Option<f64>, alignment: Option<String>, vertical_alignment: Option<String>, app: AppHandle) -> Result<(), String>);
async_command_passthrough!(pet_domain, save_pet_position(x: i32, y: i32, vertical_alignment: Option<String>) -> Result<(), String>);
async_command_passthrough!(pet_domain, save_pet_vertical_alignment(alignment: String) -> Result<(), String>);
async_command_passthrough!(pet_domain, open_main_from_pet(session_id: Option<String>, scheduled_run: Option<PetScheduledRunNavigation>, navigation: State<'_, PetNavigationState>, app: AppHandle) -> Result<(), String>);
async_command_passthrough!(pet_domain, take_pet_navigation(navigation: State<'_, PetNavigationState>) -> Result<Option<PetNavigationRequest>, String>);
async_command_passthrough!(pet_domain, queue_pet_reply(request_id: String, session_id: String, text: String, replies: State<'_, PetReplyState>, app: AppHandle) -> Result<(), String>);
async_command_passthrough!(pet_domain, take_pet_reply(replies: State<'_, PetReplyState>) -> Result<Option<PetReplyRequest>, String>);

sync_command_passthrough!(selected_pet_domain, get_selected_pet(store: State<'_, SelectedPetStore>) -> String);
sync_command_passthrough!(selected_pet_domain, set_selected_pet(id: String, expected_current: Option<String>, store: State<'_, SelectedPetStore>, app: AppHandle) -> Result<(), String>);
