//! 插件包导入端到端集成测试（可独立运行；绕开本机 lib 单测二进制 0xc0000139 启动问题）。
//!
//! 覆盖整条链路：zip 导入 → 落盘 `bundles/<id>/` → install() 写 mcp.json + installed.json。
//! 跑法：
//!   cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml \
//!       --test plugin_import_e2e -- --nocapture
//!   （4 条用例共享 PINVOU3_HOME env，进程内已用 Mutex 串行化，可并行跑）

use std::io::Write;

/// 用例间共享 PINVOU3_HOME 环境变量：同 target 默认并行跑，env 互相覆盖会竞态
/// （三轮评审）——进程内 std Mutex 串行化（与 lib 单测的 ENV_LOCK 同范式，不新增依赖）。
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 把 PINVOU3_HOME 指到干净临时目录跑闭包，跑完恢复并清理。
fn with_temp_home<F: FnOnce()>(f: F) {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let dir = std::env::temp_dir().join(format!(
        "pinvou-plugin-e2e-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let prev = std::env::var("PINVOU3_HOME").ok();
    // SAFETY: 持本文件 ENV_LOCK,测试进程内 env 写已串行化。
    unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
    f();
    match prev {
        // SAFETY: 持本文件 ENV_LOCK,测试进程内 env 写已串行化。
        Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
        // SAFETY: 持本文件 ENV_LOCK,测试进程内 env 写已串行化。
        None => unsafe { std::env::remove_var("PINVOU3_HOME") },
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// 组合包（mcp + skill + plugin.json）：落盘 + install() 供给 mcp.json/installed.json。
#[test]
fn combo_import_lands_and_registers_mcp() {
    with_temp_home(|| {
        let home = std::env::var("PINVOU3_HOME").unwrap();
        let home = std::path::Path::new(&home);
        let zip_path = home.join("combo.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("plugin.json", opts).unwrap();
            zw.write_all(
                r#"{"manifest_version":1,"id":"demo","name":"演示组合包","components":{"mcp_servers":[{"id":"demo","dir":"mcp"}],"skills":[{"id":"demo","dir":"skills/demo"}]}}"#
                    .as_bytes(),
            )
            .unwrap();
            zw.start_file("mcp/manifest.json", opts).unwrap();
            zw.write_all(
                r#"{"id":"demo","name":"演示组合包","description":"d","version":"1.0.0","icon":"","category":"life","mcp_tools":[],"command":"python","args":["server.py"],"companion_skills":["demo"]}"#
                    .as_bytes(),
            )
            .unwrap();
            zw.start_file("mcp/server.py", opts).unwrap();
            zw.write_all(b"import json\nprint(json.dumps({'ok': True}))")
                .unwrap();
            zw.start_file("skills/demo/SKILL.md", opts).unwrap();
            zw.write_all(b"---\nname: demo\n---\n# hi").unwrap();
            zw.finish().unwrap();
        }

        let report = pinvou3_lib::features::marketplace::plugin_import::import_plugin_package(
            &zip_path.to_string_lossy(),
            "combo.zip",
        )
        .unwrap();
        assert_eq!(report.id, "demo");

        // 1) 落盘
        let pkg = home.join("bundles").join("demo");
        assert!(
            pkg.join("mcp/manifest.json").is_file(),
            "mcp manifest 应落盘"
        );
        assert!(pkg.join("mcp/server.py").is_file(), "server.py 应落盘");
        assert!(pkg.join("skills/demo/SKILL.md").is_file(), "skill 应落盘");
        assert!(pkg.join("plugin.json").is_file(), "plugin.json 应落盘");
        assert!(pkg.join("icon.svg").is_file(), "icon.svg 应落盘");

        // 2) install() 供给：installed.json + mcp.json（底座据此拉起 server）。
        let mgr = pinvou3_lib::features::marketplace::MarketplaceManager::new();
        let installed = mgr.installed_ids();
        assert!(
            installed.contains(&"demo".to_string()),
            "installed.json 应含 demo，实际: {installed:?}"
        );
        let mcp_raw = std::fs::read_to_string(pinvou3_lib::platform::paths::mcp_config_path())
            .unwrap_or_default();
        assert!(
            mcp_raw.contains("\"demo\""),
            "mcp.json 应含 demo server 供给，实际: {mcp_raw}"
        );
        // 底座拉起契约：command 非空 + args 指向落盘后的 server.py（绝对路径）。
        let mcp: serde_json::Value = serde_json::from_str(&mcp_raw).unwrap();
        let server = &mcp["servers"]["demo"];
        assert!(
            server["command"]
                .as_str()
                .map(|s| !s.is_empty())
                .unwrap_or(false),
            "mcp.json 的 command 应非空，实际: {server:?}"
        );
        let args = server["args"]
            .as_array()
            .expect("mcp.json 的 args 应为数组");
        assert!(
            args.iter().any(|a| a
                .as_str()
                .map(|s| s.ends_with("server.py"))
                .unwrap_or(false)),
            "mcp.json 的 args 应含 server.py 绝对路径，实际: {args:?}"
        );
    });
}

/// 裸技能包（无 plugin.json，SKILL.md 在命名目录下）→ 规范化为 skills/<name>/。
#[test]
fn bare_skill_import_lands_canonical_layout() {
    with_temp_home(|| {
        let home = std::env::var("PINVOU3_HOME").unwrap();
        let home = std::path::Path::new(&home);
        let zip_path = home.join("skill.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("my-skill/SKILL.md", opts).unwrap();
            zw.write_all(b"---\nname: greet\n---\n# hi").unwrap();
            zw.finish().unwrap();
        }

        let report = pinvou3_lib::features::marketplace::plugin_import::import_plugin_package(
            &zip_path.to_string_lossy(),
            "skill.zip",
        )
        .unwrap();
        assert_eq!(report.id, "greet");
        let pkg = home.join("bundles").join("greet");
        assert!(
            pkg.join("skills/greet/SKILL.md").is_file(),
            "裸技能应规范化为 skills/greet/SKILL.md"
        );
        assert!(pkg.join("plugin.json").is_file(), "派生 plugin.json 应落盘");
    });
}

/// 裸 MCP 包（无 plugin.json，manifest.json 在根目录）→ 规范化为 mcp/ 并注册。
#[test]
fn bare_mcp_import_lands_and_registers() {
    with_temp_home(|| {
        let home = std::env::var("PINVOU3_HOME").unwrap();
        let home = std::path::Path::new(&home);
        let zip_path = home.join("mcp.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("manifest.json", opts).unwrap();
            zw.write_all(
                r#"{"id":"wcalc","name":"计算器","description":"d","version":"1.0.0","icon":"","category":"dev","mcp_tools":[],"command":"python","args":["server.py"]}"#
                    .as_bytes(),
            )
            .unwrap();
            zw.start_file("server.py", opts).unwrap();
            zw.write_all(b"print('server')").unwrap();
            zw.finish().unwrap();
        }

        let report = pinvou3_lib::features::marketplace::plugin_import::import_plugin_package(
            &zip_path.to_string_lossy(),
            "mcp.zip",
        )
        .unwrap();
        assert_eq!(report.id, "wcalc");
        let pkg = home.join("bundles").join("wcalc");
        assert!(
            pkg.join("mcp/manifest.json").is_file(),
            "裸 MCP manifest.json 应规范化为 mcp/manifest.json"
        );
        assert!(pkg.join("mcp/server.py").is_file(), "server.py 应落盘");

        let mgr = pinvou3_lib::features::marketplace::MarketplaceManager::new();
        assert!(
            mgr.installed_ids().contains(&"wcalc".to_string()),
            "installed.json 应含 wcalc"
        );
        let mcp_raw = std::fs::read_to_string(pinvou3_lib::platform::paths::mcp_config_path())
            .unwrap_or_default();
        assert!(
            mcp_raw.contains("\"wcalc\""),
            "mcp.json 应含 wcalc server 供给，实际: {mcp_raw}"
        );
    });
}

/// Super-skill 路径在 `feat/exec-skill` 后续 commit 里加 smoke test。
/// 旧 spanner 两项 e2e（spanner_import_lands_and_registers_spanner_runner、
/// spanner_import_rejects_broken_script）随 spanner 退场删除。
/// 注：spanner_import_lands_and_registers_spanner_runner 校验 spanner_entry +
/// companion_skills + spanner_runner.py 注入 mcp.json args；老路径不再适用。
/// spanner_import_rejects_broken_script 校验 smoke test 拦 syntax error；
/// 新 skill 包的 smoke 由 skill_marketplace::install 接管（同样 fail-fast）。

/// 单个 SKILL.md 文件包装后的形态（zip 根只放一个 SKILL.md）→ 裸技能回退识别并落盘。
/// 这是 import_skill_md_bytes 底层走的路。
#[test]
fn root_skill_md_import_lands_canonical_layout() {
    with_temp_home(|| {
        let home = std::env::var("PINVOU3_HOME").unwrap();
        let home = std::path::Path::new(&home);
        let zip_path = home.join("single.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("SKILL.md", opts).unwrap();
            zw.write_all(b"---\nname: greet\n---\n# hi").unwrap();
            zw.finish().unwrap();
        }

        let report = pinvou3_lib::features::marketplace::plugin_import::import_plugin_package(
            &zip_path.to_string_lossy(),
            "single.zip",
        )
        .unwrap();
        assert_eq!(report.id, "greet");
        let pkg = home.join("bundles").join("greet");
        assert!(
            pkg.join("skills/greet/SKILL.md").is_file(),
            "根 SKILL.md 应规范化为 skills/greet/SKILL.md"
        );
        assert!(pkg.join("plugin.json").is_file(), "派生 plugin.json 应落盘");
        assert!(pkg.join("icon.svg").is_file(), "缺省图标应落盘");
    });
}
