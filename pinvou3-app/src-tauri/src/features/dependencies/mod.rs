mod platform;

/// `progress` 回调透传给平台适配器(纯 Rust,不依赖 Tauri);由 file_ingest
/// 域层把 `app.emit("deps:install_progress", …)` 包成闭包传入。
pub fn install_dependencies(
    packages: Vec<String>,
    progress: Option<&dyn Fn(&str, usize, usize, Option<&str>)>,
) -> Result<(), String> {
    platform::install_dependencies(packages, progress)
}
