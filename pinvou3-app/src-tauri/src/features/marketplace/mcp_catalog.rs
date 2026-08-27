//! 内置 MCP 包资源目录（编译期内嵌）——可用包清单与包内容的单一真相源。
//!
//! marketplace-unification §4：MCP 包的 server 脚本与 manifest 属包内容，
//! 未安装时只住这里（只读嵌入资源），安装时才释放到 `bundles/<id>/mcp/`；
//! 启动不再全量释放到旧布局 `bundle/mcp-servers/`。
//! 远程 OAuth MCP（qcc/yuandian-mcp/canva-mcp/patsnap-search）只有 manifest，
//! 无本地脚本，同样在目录里登记（安装时释放 manifest 使包目录自描述）。
//! 每个包另带 checked-in 的包级 `plugin.json`（plugin-package-spec §3，组合包
//! 声明全量组件），释放时落到 `bundles/<id>/plugin.json` 与 mcp/ 同级——运行层
//! 不读其内容，落盘纯为包自描述/导出准备。

use std::path::{Path, PathBuf};

use crate::platform::paths;

/// 一个内嵌 MCP 包：manifest + 包内文件（server 脚本等，相对 `mcp/` 目录）+
/// 包级 plugin.json（落 `bundles/<id>/plugin.json`，与 mcp/、skills/ 同级，
/// plugin-package-spec §3；运行层不读其内容，落盘纯为包自描述/导出准备）。
pub struct McpPackageSpec {
    pub id: &'static str,
    pub manifest_json: &'static str,
    pub plugin_json: &'static str,
    pub files: &'static [(&'static str, &'static str)],
}

/// 内嵌目录（单一真相源）。`MarketplaceManager::available_tools` 的清单来源、
/// install 的释放来源、启动 sync 的比对基准都从这里取。
pub const MCP_PACKAGES: &[McpPackageSpec] = &[
    McpPackageSpec {
        id: "weather",
        manifest_json: include_str!("../../../../resources/mcp-servers/weather/manifest.json"),
        plugin_json: include_str!("../../../../resources/mcp-servers/weather/plugin.json"),
        files: &[(
            "server.py",
            include_str!("../../../../resources/mcp-servers/weather/server.py"),
        )],
    },
    McpPackageSpec {
        id: "iwencai",
        manifest_json: include_str!("../../../../resources/mcp-servers/iwencai/manifest.json"),
        plugin_json: include_str!("../../../../resources/mcp-servers/iwencai/plugin.json"),
        files: &[(
            "server.py",
            include_str!("../../../../resources/mcp-servers/iwencai/server.py"),
        )],
    },
    // 远程 MCP：只有 manifest，无本地脚本
    McpPackageSpec {
        id: "qcc",
        manifest_json: include_str!("../../../../resources/mcp-servers/qcc/manifest.json"),
        plugin_json: include_str!("../../../../resources/mcp-servers/qcc/plugin.json"),
        files: &[],
    },
    McpPackageSpec {
        id: "yuandian-mcp",
        manifest_json: include_str!("../../../../resources/mcp-servers/yuandian-mcp/manifest.json"),
        plugin_json: include_str!("../../../../resources/mcp-servers/yuandian-mcp/plugin.json"),
        files: &[],
    },
    McpPackageSpec {
        id: "canva-mcp",
        manifest_json: include_str!("../../../../resources/mcp-servers/canva-mcp/manifest.json"),
        plugin_json: include_str!("../../../../resources/mcp-servers/canva-mcp/plugin.json"),
        files: &[],
    },
    McpPackageSpec {
        id: "patsnap-search",
        manifest_json: include_str!(
            "../../../../resources/mcp-servers/patsnap-search/manifest.json"
        ),
        plugin_json: include_str!("../../../../resources/mcp-servers/patsnap-search/plugin.json"),
        files: &[],
    },
    McpPackageSpec {
        id: "obsidian",
        manifest_json: include_str!("../../../../resources/mcp-servers/obsidian/manifest.json"),
        plugin_json: include_str!("../../../../resources/mcp-servers/obsidian/plugin.json"),
        files: &[(
            "server.py",
            include_str!("../../../../resources/mcp-servers/obsidian/server.py"),
        )],
    },
    McpPackageSpec {
        id: "pptx",
        manifest_json: include_str!("../../../../resources/mcp-servers/pptx/manifest.json"),
        plugin_json: include_str!("../../../../resources/mcp-servers/pptx/plugin.json"),
        files: &[(
            "server.py",
            include_str!("../../../../resources/mcp-servers/pptx/server.py"),
        )],
    },
    McpPackageSpec {
        id: "gongwen",
        manifest_json: include_str!("../../../../resources/mcp-servers/gongwen/manifest.json"),
        plugin_json: include_str!("../../../../resources/mcp-servers/gongwen/plugin.json"),
        files: &[
            (
                "server.py",
                include_str!("../../../../resources/mcp-servers/gongwen/server.py"),
            ),
            (
                "gbt9704_styles.py",
                include_str!("../../../../resources/mcp-servers/gongwen/gbt9704_styles.py"),
            ),
        ],
    },
    // 腾讯文档（官方远程 MCP ×4，个人 Token 经无 scheme Authorization 头注入
    // ——官方端点要求原始 Token，不能加 Bearer 前缀）。
    McpPackageSpec {
        id: "tencent-docs",
        manifest_json: include_str!("../../../../resources/mcp-servers/tencent-docs/manifest.json"),
        plugin_json: include_str!("../../../../resources/mcp-servers/tencent-docs/plugin.json"),
        files: &[],
    },
    // 企微群机器人（本地 stdio，包装企业微信官方群机器人 webhook 消息推送 API；
    // key 走凭据库 + ${ENV} 占位符，不落明文）。
    McpPackageSpec {
        id: "wecom-bot",
        manifest_json: include_str!("../../../../resources/mcp-servers/wecom-bot/manifest.json"),
        plugin_json: include_str!("../../../../resources/mcp-servers/wecom-bot/plugin.json"),
        files: &[(
            "server.py",
            include_str!("../../../../resources/mcp-servers/wecom-bot/server.py"),
        )],
    },
];

/// 按 id 取内嵌包（不在目录 = 自定义/手放工具，走旧布局回退）。
pub fn spec_for(id: &str) -> Option<&'static McpPackageSpec> {
    MCP_PACKAGES.iter().find(|spec| spec.id == id)
}

/// 包的 mcp/ 目录（bundles/<id>/mcp/）。
pub fn package_mcp_dir(id: &str) -> PathBuf {
    paths::bundles_root().join(id).join("mcp")
}

/// 释放包内容到 `bundles/<id>/mcp/`（staged + 原子 rename，与技能 install 同范式），
/// 并把包级 plugin.json 写到 `bundles/<id>/plugin.json`（与 mcp/ 同级——组合包的
/// skills/ 由技能管线随后落入，plugin.json 按规范 §3 预先声明全量组件；运行层不读
/// 其内容，落盘纯为包自描述/导出准备）。
/// 返回包内容指纹（写 BundleStore 记录用）；不在内嵌目录 → Ok(None)（调用方回退
/// 旧布局）。内容指纹只覆盖 mcp/ 子树（跳过旧布局残留标记，与技能指纹同一口径），
/// plugin.json 是派生自描述文件，不参与指纹。
pub fn release_package(id: &str) -> Result<Option<String>, String> {
    let Some(spec) = spec_for(id) else {
        return Ok(None);
    };
    let mcp_dir = package_mcp_dir(id);
    let parent = mcp_dir.parent().expect("包目录必有父级");
    std::fs::create_dir_all(parent).map_err(|e| format!("创建包目录失败: {e}"))?;
    let staged = parent.join(".mcp.tmp");
    let _ = std::fs::remove_dir_all(&staged);
    std::fs::create_dir_all(&staged).map_err(|e| format!("创建暂存目录: {e}"))?;

    let result = (|| -> Result<String, String> {
        std::fs::write(staged.join("manifest.json"), spec.manifest_json)
            .map_err(|e| format!("写 manifest: {e}"))?;
        for (name, content) in spec.files {
            std::fs::write(staged.join(name), content).map_err(|e| format!("写 {name}: {e}"))?;
        }
        super::skill_marketplace::dir_fingerprint(&staged)
    })();
    let fingerprint = match result {
        Ok(fp) => fp,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staged);
            return Err(e);
        }
    };
    // 删旧目录不得吞错误：Windows 文件锁导致删一半时若继续 rename 必然失败，
    // 且暂存被清理后盘上只剩残缺的旧目录（静默损坏残留）。失败即中止并保留
    // 原目录现状，下个启动周期重试收敛。
    if mcp_dir.exists() {
        std::fs::remove_dir_all(&mcp_dir).map_err(|e| {
            let _ = std::fs::remove_dir_all(&staged);
            format!("清理旧包目录失败（已中止，原目录可能部分删除）: {e}")
        })?;
    }
    std::fs::rename(&staged, &mcp_dir).map_err(|e| {
        let _ = std::fs::remove_dir_all(&staged);
        format!("释放包资源: {e}")
    })?;
    write_plugin_json_if_changed(spec)?;
    Ok(Some(fingerprint))
}

/// 包级 plugin.json 落盘到 `bundles/<id>/plugin.json`（内容一致则零写盘）。
/// 返回是否实际写入。
fn write_plugin_json_if_changed(spec: &McpPackageSpec) -> Result<bool, String> {
    let path = paths::bundles_root().join(spec.id).join("plugin.json");
    if std::fs::read(&path).is_ok_and(|bytes| bytes == spec.plugin_json.as_bytes()) {
        return Ok(false);
    }
    std::fs::write(&path, spec.plugin_json).map_err(|e| format!("写 plugin.json: {e}"))?;
    Ok(true)
}

/// 启动校验/补齐：包目录内容应与内嵌目录一致（manifest + 全部文件逐字节比对，
/// 不同才重写），包级 plugin.json 缺失/过期也一并补齐（旧版安装只有 mcp/ 子树
/// 没有 plugin.json 的升级路径在此覆盖）。全部一致 → Ok(false)，零写盘。指纹在
/// 不一致重写后由 [`release_package`] 返回的口径现算并补写进既有 BundleStore 记录。
///
/// 这也是旧布局的存量迁移路径（§9：重释放优先——内容指纹可对，优于逐文件 move）。
pub fn ensure_package_released(id: &str) -> Result<bool, String> {
    let Some(spec) = spec_for(id) else {
        return Ok(false);
    };
    let mcp_dir = package_mcp_dir(id);
    let mut changed = false;
    if !package_dir_matches(spec, &mcp_dir) {
        let Some(fingerprint) = release_package(id)? else {
            return Ok(false);
        };
        // 补写/订正指纹到既有记录（install 路径已写过同值，这里是迁移/自愈路径）
        let store = super::store::BundleStore::new();
        match store.get(id) {
            Ok(Some(mut record)) if record.content_fingerprint.as_deref() != Some(&fingerprint) => {
                record.content_fingerprint = Some(fingerprint);
                if let Err(e) = store.upsert_preserving(record) {
                    log::warn!("[mcp-catalog] 补写包指纹失败（{id}）: {e}");
                }
            }
            Ok(_) => {}
            Err(e) => log::warn!("[mcp-catalog] 读取 BundleStore 失败（{id}）: {e}"),
        }
        changed = true;
    }
    // plugin.json 收敛（release_package 已写过时此处为零写盘 no-op）
    if write_plugin_json_if_changed(spec)? {
        changed = true;
    }
    Ok(changed)
}

/// 包目录与内嵌内容逐字节比对（manifest + 全部文件；多出/缺失/不同都算不一致）。
fn package_dir_matches(spec: &McpPackageSpec, dir: &Path) -> bool {
    let expected: Vec<(&str, &str)> = std::iter::once(("manifest.json", spec.manifest_json))
        .chain(spec.files.iter().copied())
        .collect();
    for (name, content) in &expected {
        let Ok(on_disk) = std::fs::read_to_string(dir.join(name)) else {
            return false;
        };
        if on_disk != *content {
            return false;
        }
    }
    // 目录里不应有预期之外的文件（旧版残留等）
    let Ok(rd) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_file() && !expected.iter().any(|(n, _)| *n == name) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 内嵌目录完整性：每个 id 的 manifest 可解析且 id 一致；文件非空。
    #[test]
    fn catalog_manifests_parse_and_match_ids() {
        for spec in MCP_PACKAGES {
            let manifest: serde_json::Value = serde_json::from_str(spec.manifest_json)
                .unwrap_or_else(|e| panic!("{} manifest 解析失败: {e}", spec.id));
            assert_eq!(
                manifest["id"].as_str(),
                Some(spec.id),
                "{} manifest id 与目录条目不一致",
                spec.id
            );
            for (name, content) in spec.files {
                assert!(!content.is_empty(), "{} 的 {name} 为空", spec.id);
            }
        }
        // 已知 11 个内置包
        assert_eq!(MCP_PACKAGES.len(), 11);
    }

    /// checked-in plugin.json 与包内容一致（plugin-package-spec §3）：
    /// manifest_version=1、id/name/version/description 与 manifest.json 同源、
    /// components 声明与包实际组件一致（mcp_servers 恰一个且 dir="mcp"；
    /// skills 恰为 manifest companion_skills 落盘的技能目录）。
    #[test]
    fn catalog_plugin_json_matches_package_content() {
        let skill_mgr =
            crate::features::marketplace::skill_marketplace::SkillMarketplaceManager::new();
        for spec in MCP_PACKAGES {
            let tool: crate::features::marketplace::ToolManifest =
                serde_json::from_str(spec.manifest_json)
                    .unwrap_or_else(|e| panic!("{} manifest 解析失败: {e}", spec.id));
            let plugin: crate::features::marketplace::plugin_import::PluginManifest =
                serde_json::from_str(spec.plugin_json)
                    .unwrap_or_else(|e| panic!("{} plugin.json 解析失败: {e}", spec.id));
            assert_eq!(plugin.manifest_version, 1, "{} manifest_version", spec.id);
            assert_eq!(plugin.id, spec.id, "{} plugin id", spec.id);
            assert!(
                crate::features::marketplace::plugin_import::is_safe_component_id(&plugin.id),
                "{} 包 id 字符集",
                spec.id
            );
            assert_eq!(
                plugin.name, tool.name,
                "{} name 应与 manifest 一致",
                spec.id
            );
            assert_eq!(
                plugin.version.as_deref(),
                Some(tool.version.as_str()),
                "{} version 应与 manifest 一致",
                spec.id
            );
            assert_eq!(
                plugin.description.as_deref(),
                Some(tool.description.as_str()),
                "{} description 应与 manifest 一致",
                spec.id
            );
            let comps = plugin
                .components
                .as_ref()
                .unwrap_or_else(|| panic!("{} 缺 components", spec.id));
            // MCP 组件：恰一个，id 与 manifest 一致，规范化 dir
            assert_eq!(comps.mcp_servers.len(), 1, "{} mcp_servers 数量", spec.id);
            assert_eq!(comps.mcp_servers[0].id, tool.id, "{} mcp 组件 id", spec.id);
            assert_eq!(comps.mcp_servers[0].dir, "mcp", "{} mcp 组件 dir", spec.id);
            // 技能组件：恰为 companion_skills 落盘的技能目录（市场 id → 技能名）
            let expected_skills = skill_mgr.model_skill_names(&tool.companion_skills);
            let declared: Vec<&str> = comps.skills.iter().map(|c| c.id.as_str()).collect();
            let mut expected: Vec<&str> = expected_skills.iter().map(|s| s.as_str()).collect();
            let mut declared_sorted = declared.clone();
            declared_sorted.sort_unstable();
            expected.sort_unstable();
            assert_eq!(
                declared_sorted, expected,
                "{} skills 声明应与 companion_skills 落盘一致",
                spec.id
            );
            for c in &comps.skills {
                assert_eq!(
                    c.dir,
                    format!("skills/{}", c.id),
                    "{} 技能组件 dir 非规范",
                    spec.id
                );
            }
        }
    }

    /// 把 PINVOU3_HOME 指到干净临时目录跑闭包，跑完恢复并清理。
    /// 与 marketplace 其他 env-mutating 测试共享 ENV_LOCK 串行。
    fn with_temp_home<F: FnOnce()>(f: F) {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir =
            std::env::temp_dir().join(format!("pinvou3-mcp-catalog-test-{}", std::process::id()));
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

    /// 安装释放路径：release_package 落 mcp/ 内容 + 包级 plugin.json
    /// （与 mcp/ 同级，内容与内嵌一致）。
    #[test]
    fn release_package_lands_plugin_json() {
        with_temp_home(|| {
            let fp = release_package("weather").unwrap();
            assert!(fp.is_some());
            let pkg = paths::bundles_root().join("weather");
            assert!(pkg.join("mcp/manifest.json").is_file());
            let on_disk = std::fs::read_to_string(pkg.join("plugin.json")).unwrap();
            assert_eq!(on_disk, spec_for("weather").unwrap().plugin_json);
        });
    }

    /// 升级路径：旧版安装只有 mcp/ 子树（无 plugin.json）时，
    /// ensure_package_released 补齐 plugin.json 且不触碰已一致的 mcp/。
    #[test]
    fn ensure_package_released_backfills_plugin_json() {
        with_temp_home(|| {
            // 模拟旧版安装：只释放 mcp/ 内容，不写 plugin.json
            let spec = spec_for("weather").unwrap();
            let mcp_dir = package_mcp_dir("weather");
            std::fs::create_dir_all(&mcp_dir).unwrap();
            std::fs::write(mcp_dir.join("manifest.json"), spec.manifest_json).unwrap();
            for (name, content) in spec.files {
                std::fs::write(mcp_dir.join(name), content).unwrap();
            }
            assert!(ensure_package_released("weather").unwrap());
            let pkg = paths::bundles_root().join("weather");
            assert_eq!(
                std::fs::read_to_string(pkg.join("plugin.json")).unwrap(),
                spec.plugin_json
            );
            // 全部一致后零写盘
            assert!(!ensure_package_released("weather").unwrap());
        });
    }
}
