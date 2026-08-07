mod platform;

/// `progress` 回调透传给平台适配器(纯 Rust,不依赖 Tauri);由 file_ingest
/// 域层把 `app.emit("deps:install_progress", …)` 包成闭包传入。
///
/// 回调标 `+ Sync`:macOS 侧用两个作用域线程并发排空 stdout/stderr 并各自调用
/// 回调,因此回调必须可跨线程共享调用。其余平台单线程调用,同样满足。
pub fn install_dependencies(
    packages: Vec<String>,
    progress: Option<&(dyn Fn(&str, usize, usize, Option<&str>) + Sync)>,
) -> Result<(), String> {
    platform::install_dependencies(packages, progress)
}
