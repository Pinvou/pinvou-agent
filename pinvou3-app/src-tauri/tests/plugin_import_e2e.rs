//! 插件包导入端到端集成测试（可独立运行；绕开本机 lib 单测二进制 0xc0000139 启动问题）。
//!
//! 覆盖整条链路：zip 导入 → 落盘 `bundles/<id>/` → install() 写 mcp.json + installed.json。
//! 跑法：
//!   cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml \
//!       --test plugin_import_e2e -- --nocapture --test-threads=1

use std::io::Write;

/// 把 PINVOU3_HOME 指到干净临时目录跑闭包，跑完恢复并清理。集成测试独立进程、
/// 串行跑，无需与 lib 单测共享 ENV_LOCK。
fn with_temp_home<F: FnOnce()>(f: F) {
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
    std::env::set_var("PINVOU3_HOME", &dir);
    f();
    match prev {
        Some(v) => std::env::set_var("PINVOU3_HOME", v),
        None => std::env::remove_var("PINVOU3_HOME"),
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

/// spanner 扳手插件包：落盘 spanner/ + 合成 mcp/manifest.json + 供给 spanner_runner。
/// spanner + skill 组合包：配套技能声明 companion_skills，让技能引导与工具同卡。
#[test]
fn spanner_import_lands_and_registers_spanner_runner() {
    with_temp_home(|| {
        let home = std::env::var("PINVOU3_HOME").unwrap();
        let home = std::path::Path::new(&home);
        let zip_path = home.join("spanner.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("plugin.json", opts).unwrap();
            zw.write_all(
                r#"{"manifest_version":1,"id":"hello","name":"Hello","version":"1.0.0","spanner":{"entry":"main.py","input_schema":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}},"components":{"skills":[{"id":"hello","dir":"skills/hello"}]}}"#
                    .as_bytes(),
            )
            .unwrap();
            zw.start_file("spanner/main.py", opts).unwrap();
            zw.write_all(b"import json,sys\njson.dump({'ok': json.load(sys.stdin)}, sys.stdout)")
                .unwrap();
            zw.start_file("skills/hello/SKILL.md", opts).unwrap();
            zw.write_all(b"---\nname: hello\n---\n# hello").unwrap();
            zw.finish().unwrap();
        }

        let report = pinvou3_lib::features::marketplace::plugin_import::import_plugin_package(
            &zip_path.to_string_lossy(),
            "spanner.zip",
        )
        .unwrap();
        assert_eq!(report.id, "hello");
        let pkg = home.join("bundles").join("hello");
        assert!(pkg.join("spanner/main.py").is_file(), "spanner 入口应落盘");
        assert!(
            pkg.join("skills/hello/SKILL.md").is_file(),
            "配套技能应落盘"
        );
        assert!(
            pkg.join("mcp/manifest.json").is_file(),
            "合成 mcp manifest 应落盘"
        );
        assert!(pkg.join("plugin.json").is_file(), "plugin.json 应落盘");
        assert!(pkg.join("icon.svg").is_file(), "缺省图标应落盘");

        // 合成 manifest 应带 spanner_entry + companion_skills（技能与工具同卡/同开关）。
        let synth: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(pkg.join("mcp/manifest.json")).unwrap())
                .unwrap();
        assert_eq!(
            synth["spanner_entry"], "main.py",
            "合成 manifest 应带 spanner_entry"
        );
        let companions = synth["companion_skills"].as_array().unwrap();
        assert!(
            companions.iter().any(|c| c.as_str() == Some("hello")),
            "合成 manifest 应声明 companion_skills=hello，实际: {companions:?}"
        );

        // 供给：mcp.json 的 args 应指向 spanner_runner.py + plugin.json（非 server.py）。
        let mgr = pinvou3_lib::features::marketplace::MarketplaceManager::new();
        assert!(
            mgr.installed_ids().contains(&"hello".to_string()),
            "installed.json 应含 hello"
        );
        let mcp_raw = std::fs::read_to_string(pinvou3_lib::platform::paths::mcp_config_path())
            .unwrap_or_default();
        let mcp: serde_json::Value = serde_json::from_str(&mcp_raw).unwrap();
        let server = &mcp["servers"]["hello"];
        let args = server["args"].as_array().expect("args 应为数组");
        assert!(
            args.iter().any(|a| a
                .as_str()
                .map(|s| s.ends_with("spanner_runner.py"))
                .unwrap_or(false)),
            "spanner 供给的 args 应含 spanner_runner.py，实际: {args:?}"
        );
        assert!(
            args.iter().any(|a| a
                .as_str()
                .map(|s| s.ends_with("plugin.json"))
                .unwrap_or(false)),
            "spanner 供给的 args 应含 plugin.json，实际: {args:?}"
        );
    });
}

/// 安装时 smoke test：入口脚本语法错误 → 导入应被拒绝（不留半安装状态）。
#[test]
fn spanner_import_rejects_broken_script() {
    with_temp_home(|| {
        let home = std::env::var("PINVOU3_HOME").unwrap();
        let home = std::path::Path::new(&home);
        let zip_path = home.join("broken.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("plugin.json", opts).unwrap();
            zw.write_all(
                r#"{"manifest_version":1,"id":"broken","name":"Broken","version":"1.0.0","spanner":{"entry":"main.py","input_schema":{"type":"object","properties":{}}}}"#
                    .as_bytes(),
            )
            .unwrap();
            zw.start_file("spanner/main.py", opts).unwrap();
            // 故意写坏：语法错误
            zw.write_all(b"this is not valid python !!!").unwrap();
            zw.finish().unwrap();
        }

        let err = pinvou3_lib::features::marketplace::plugin_import::import_plugin_package(
            &zip_path.to_string_lossy(),
            "broken.zip",
        )
        .unwrap_err();
        assert!(
            err.contains("安装自检失败") || err.contains("smoke test"),
            "应返回安装自检失败，实际: {err}"
        );
        // 不留半安装：包目录不应存在
        assert!(
            !home.join("bundles").join("broken").exists(),
            "broken 包不应落盘"
        );
    });
}

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
