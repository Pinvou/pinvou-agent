//! 内置 MCP 包资源目录（编译期内嵌）——可用包清单与包内容的单一真相源。
//!
//! marketplace-unification §4：MCP 包的 server 脚本与 manifest 属包内容，
//! 未安装时只住这里（只读嵌入资源），安装时才释放到 `bundles/<id>/mcp/`；
//! 启动不再全量释放到旧布局 `bundle/mcp-servers/`。
//! 远程 OAuth MCP（qcc/yuandian-mcp/canva-mcp/patsnap-search）只有 manifest，
//! 无本地脚本，同样在目录里登记（安装时释放 manifest 使包目录自描述）。

use std::path::{Path, PathBuf};

use crate::platform::paths;

/// 一个内嵌 MCP 包：manifest + 包内文件（server 脚本等，相对 `mcp/` 目录）。
pub struct McpPackageSpec {
    pub id: &'static str,
    pub manifest_json: &'static str,
    pub files: &'static [(&'static str, &'static str)],
}

/// 内嵌目录（单一真相源）。`MarketplaceManager::available_tools` 的清单来源、
/// install 的释放来源、启动 sync 的比对基准都从这里取。
pub const MCP_PACKAGES: &[McpPackageSpec] = &[
    McpPackageSpec {
        id: "weather",
        manifest_json: include_str!("../../../../resources/mcp-servers/weather/manifest.json"),
        files: &[(
            "server.py",
            include_str!("../../../../resources/mcp-servers/weather/server.py"),
        )],
    },
    McpPackageSpec {
        id: "iwencai",
        manifest_json: include_str!("../../../../resources/mcp-servers/iwencai/manifest.json"),
        files: &[(
            "server.py",
            include_str!("../../../../resources/mcp-servers/iwencai/server.py"),
        )],
    },
    // 远程 MCP：只有 manifest，无本地脚本
    McpPackageSpec {
        id: "qcc",
        manifest_json: include_str!("../../../../resources/mcp-servers/qcc/manifest.json"),
        files: &[],
    },
    McpPackageSpec {
        id: "yuandian-mcp",
        manifest_json: include_str!("../../../../resources/mcp-servers/yuandian-mcp/manifest.json"),
        files: &[],
    },
    McpPackageSpec {
        id: "canva-mcp",
        manifest_json: include_str!("../../../../resources/mcp-servers/canva-mcp/manifest.json"),
        files: &[],
    },
    McpPackageSpec {
        id: "patsnap-search",
        manifest_json: include_str!(
            "../../../../resources/mcp-servers/patsnap-search/manifest.json"
        ),
        files: &[],
    },
    McpPackageSpec {
        id: "obsidian",
        manifest_json: include_str!("../../../../resources/mcp-servers/obsidian/manifest.json"),
        files: &[(
            "server.py",
            include_str!("../../../../resources/mcp-servers/obsidian/server.py"),
        )],
    },
    McpPackageSpec {
        id: "pptx",
        manifest_json: include_str!("../../../../resources/mcp-servers/pptx/manifest.json"),
        files: &[(
            "server.py",
            include_str!("../../../../resources/mcp-servers/pptx/server.py"),
        )],
    },
    McpPackageSpec {
        id: "gongwen",
        manifest_json: include_str!("../../../../resources/mcp-servers/gongwen/manifest.json"),
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
        files: &[],
    },
    // 企微群机器人（本地 stdio，包装企业微信官方群机器人 webhook 消息推送 API；
    // key 走凭据库 + ${ENV} 占位符，不落明文）。
    McpPackageSpec {
        id: "wecom-bot",
        manifest_json: include_str!("../../../../resources/mcp-servers/wecom-bot/manifest.json"),
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

/// 释放包内容到 `bundles/<id>/mcp/`（staged + 原子 rename，与技能 install 同范式）。
/// 返回包内容指纹（写 BundleStore 记录用）；不在内嵌目录 → Ok(None)（调用方回退
/// 旧布局）。内容指纹跳过旧布局残留标记，与技能指纹同一口径。
pub fn release_package(id: &str) -> Result<Option<String>, String> {
    let Some(spec) = spec_for(id) else {
        return Ok(None);
    };
    let mcp_dir = package_mcp_dir(id);
    // bundles/<id>/mcp is joined from bundles_root() and always has a parent;
    // still return an error as the fallback.
    let Some(parent) = mcp_dir.parent() else {
        return Err(format!("package dir missing parent: {}", mcp_dir.display()));
    };
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
    Ok(Some(fingerprint))
}

/// 启动校验/补齐：包目录内容应与内嵌目录一致（manifest + 全部文件逐字节比对，
/// 不同才重写）。一致 → Ok(false)，零写盘。指纹在不一致重写后由
/// [`release_package`] 返回的口径现算并补写进既有 BundleStore 记录。
///
/// 这也是旧布局的存量迁移路径（§9：重释放优先——内容指纹可对，优于逐文件 move）。
pub fn ensure_package_released(id: &str) -> Result<bool, String> {
    let Some(spec) = spec_for(id) else {
        return Ok(false);
    };
    let mcp_dir = package_mcp_dir(id);
    if package_dir_matches(spec, &mcp_dir) {
        return Ok(false);
    }
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
    Ok(true)
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
}
