//! 会话开关（disabled）与可见性（hidden）门控的集成测试。
//!
//! 与 `scope.rs` 内 lib 单测互补：本机 lib 单测二进制存在 0xc0000139 启动问题，
//! 这里走公开 API 在独立集成测试进程中验证同一份契约，可在本地直接运行：
//!   cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml \
//!       --test scope_visibility -- --nocapture --test-threads=1

use pinvou3_lib::features::marketplace::scope::{
    load_disabled_bundles_for, load_hidden_bundles_for, remove_bundle_from_disabled_scopes,
    save_disabled_bundles_for, save_hidden_bundles_for, sync_deny_all_scopes_after_install,
    unavailable_bundles_for,
};
use pinvou3_lib::features::marketplace::ConnectorScope;

/// 用例间共享 PINVOU3_HOME 环境变量：同 target 默认并行跑，env 互相覆盖会竞态
/// （六轮评审 R3）——进程内 std Mutex 串行化（与 plugin_import_e2e.rs 同范式，
/// 不新增依赖）。
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 把 PINVOU3_HOME 指到干净临时目录跑闭包，跑完恢复并清理。
fn with_temp_home<F: FnOnce()>(f: F) {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let dir = std::env::temp_dir().join(format!(
        "pinvou-scope-vis-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let prev = std::env::var("PINVOU3_HOME").ok();
    std::env::set_var("PINVOU3_HOME", &dir);
    f();
    match prev {
        Some(v) => std::env::set_var("PINVOU3_HOME", v),
        None => std::env::remove_var("PINVOU3_HOME"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// 开关与可见性正交：hidden 不影响 disabled；不可用集是两者并集且去重。
#[test]
fn hidden_is_orthogonal_and_unavailable_is_union() {
    with_temp_home(|| {
        // 初始：两套集合都空。
        assert!(load_disabled_bundles_for(ConnectorScope::Plain).is_empty());
        assert!(load_hidden_bundles_for(ConnectorScope::Plain).is_empty());

        // 关掉 weather 的开关，隐藏 weather + pptx（weather 同时出现在两套集合里）。
        save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]);
        save_hidden_bundles_for(
            ConnectorScope::Plain,
            &["weather".to_string(), "pptx".to_string()],
        );

        // 可见性写入不污染开关集合。
        assert_eq!(
            load_disabled_bundles_for(ConnectorScope::Plain),
            vec!["weather".to_string()]
        );
        assert_eq!(
            load_hidden_bundles_for(ConnectorScope::Plain),
            vec!["weather".to_string(), "pptx".to_string()]
        );

        // 并集去重：weather 只出现一次。
        let mut unavailable = unavailable_bundles_for(ConnectorScope::Plain);
        unavailable.sort();
        assert_eq!(unavailable, vec!["pptx".to_string(), "weather".to_string()]);
    });
}

/// 卸载/断开后清理残留：disabled 与 hidden 两套集合同时移除该包 id。
#[test]
fn remove_bundle_clears_both_sets() {
    with_temp_home(|| {
        save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]);
        save_hidden_bundles_for(ConnectorScope::Plain, &["weather".to_string()]);
        remove_bundle_from_disabled_scopes("weather");
        assert!(load_disabled_bundles_for(ConnectorScope::Plain).is_empty());
        assert!(load_hidden_bundles_for(ConnectorScope::Plain).is_empty());
    });
}

/// DenyAll 模式（code）默认关闭同步只进 disabled 集，不误入可见性集：新装包默认
/// 关闭但仍应可见，用户才能在商店/会话卡里看到并显式开启。
#[test]
fn sync_deny_all_after_install_does_not_hide() {
    with_temp_home(|| {
        // 写空列表即标记 code scope 已初始化（DenyAll 模式才参与默认关闭同步）。
        save_disabled_bundles_for(ConnectorScope::Code, &[]);
        sync_deny_all_scopes_after_install("weather");
        assert!(load_disabled_bundles_for(ConnectorScope::Code).contains(&"weather".to_string()));
        assert!(load_hidden_bundles_for(ConnectorScope::Code).is_empty());
    });
}
