use super::prelude::*;
use crate::platform::startup as startup_domain;
use crate::platform::window_startup as window_startup_domain;
use startup_domain::*;

sync_command_passthrough!(startup_domain, report_frontend_startup(entries: Vec<FrontendStartupEntry>));
sync_command_passthrough!(window_startup_domain, reveal_startup_window(window: tauri::WebviewWindow) -> Result<bool, String>);
