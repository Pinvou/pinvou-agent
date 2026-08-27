//! 包目录 → 标准插件包 zip 的导出：回收站导出与已安装包导出共用的写出逻辑。
//!
//! zip 布局对齐 plugin-package-spec：包内容（plugin.json、mcp/、skills/ 等）
//! 平铺在 zip 根，可经统一导入管线 `plugin_import::import_plugin_package` 重新
//! 导入。只打包插件包本体（不含回收站清单等外部元数据）；Python 运行缓存
//! （`__pycache__/`、`*.pyc`）不打包（与导入管线的磁盘比对豁免口径一致）。
//!
//! manifest 净化（导出已安装包时启用，防御性兜底）：安装期
//! `connectors::add_local_to_mcp_json` 会把 `server.py` 入口参数改写为
//! `bundles/<id>/mcp/server.py` 绝对路径 —— 但该改写只发生在写 mcp.json 时，
//! 盘上 manifest 保持原始相对形式；旧版本/手改/迁移路径落过绝对路径的包，
//! 导出时把 args 中指向包内 `mcp/` 目录的绝对路径参数还原为相对形式（入口
//! 脚本名），保证 zip 在别的机器可再导入。凭据占位符 `${PINVOU3_MCP_SECRET_*}`、
//! env、servers 等均不动。

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::platform::paths;

/// 已安装包导出为标准插件包 zip。fail-closed：id 安全校验 + 只导出
/// `bundles_root()` 下真实存在的包目录（不存在的 id 报错，不导出任何文件）。
pub fn export_installed_plugin(pkg_id: &str, dest_zip: &Path) -> Result<(), String> {
    if !super::skill_marketplace::is_safe_skill_name(pkg_id) {
        return Err(format!("非法包 id '{pkg_id}'"));
    }
    let pkg_dir = paths::bundles_root().join(pkg_id);
    if !pkg_dir.is_dir() {
        return Err(format!(
            "包 '{pkg_id}' 未安装（{} 不存在），拒绝导出",
            pkg_dir.display()
        ));
    }
    let written = write_package_zip(&pkg_dir, dest_zip, true)?;
    log::info!(
        "[package-export] 已导出已安装包 {pkg_id}（{written} 个条目）→ {}",
        dest_zip.display()
    );
    Ok(())
}

/// 把包目录打成标准插件包 zip（包内容平铺在 zip 根），返回写入条目数。
/// `sanitize_args`：`mcp/manifest.json` 的 args 净化（见模块注释；回收站导出
/// 置 false——回收的包未经安装期改写，原样打包）。
/// 直接写用户选定的目标（保存对话框已确认覆盖）；中途失败清理半写文件，
/// 不留损坏 zip。
pub(crate) fn write_package_zip(
    src_dir: &Path,
    dest_zip: &Path,
    sanitize_args: bool,
) -> Result<usize, String> {
    let mut files = Vec::new();
    collect_export_files(src_dir, src_dir, &mut files)
        .map_err(|e| format!("遍历 {} 失败: {e}", src_dir.display()))?;
    if files.is_empty() {
        return Err(format!("包目录 {} 为空，无法导出", src_dir.display()));
    }
    files.sort();
    let result = (|| -> Result<(), String> {
        let out = std::fs::File::create(dest_zip)
            .map_err(|e| format!("创建 {} 失败: {e}", dest_zip.display()))?;
        let mut zw = zip::ZipWriter::new(out);
        let opts = zip::write::SimpleFileOptions::default();
        for (rel, path) in &files {
            // 条目名即包内相对路径（盘上真实遍历产出，恒为 root 子孙，无穿越
            // 风险）；统一 '/' 分隔，与导入管线的路径口径一致。
            zw.start_file(rel, opts)
                .map_err(|e| format!("写 zip 条目 {rel}: {e}"))?;
            if sanitize_args && rel == "mcp/manifest.json" {
                let raw = std::fs::read(path)
                    .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
                let sanitized = sanitize_manifest_args(&raw, &src_dir.join("mcp"));
                zw.write_all(&sanitized)
                    .map_err(|e| format!("写 zip 条目 {rel}: {e}"))?;
            } else {
                let mut reader = std::fs::File::open(path)
                    .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
                std::io::copy(&mut reader, &mut zw)
                    .map_err(|e| format!("写 zip 条目 {rel}: {e}"))?;
            }
        }
        zw.finish().map_err(|e| format!("完成 zip 写入: {e}"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(dest_zip);
    }
    result?;
    Ok(files.len())
}

/// 递归收集导出条目（相对路径用 '/' 分隔）：跳过 Python 运行缓存
/// （`__pycache__/` 子树与 `*.pyc`，与 plugin_import 的磁盘比对豁免口径一致）。
fn collect_export_files(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(String, PathBuf)>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_export_files(root, &path, out)?;
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if rel.split('/').any(|c| c == "__pycache__") || rel.to_ascii_lowercase().ends_with(".pyc")
        {
            continue;
        }
        out.push((rel, path));
    }
    Ok(())
}

/// manifest args 净化：指向包内 `mcp/` 目录的绝对路径参数还原为相对形式
/// （'/' 分隔，与安装期改写判定的 `ends_with("/server.py")` 口径兼容）；
/// 其余参数（相对形式、指向包外的绝对路径）不动。manifest 解析失败/无 args/
/// 无需改写 → 返回原始字节（导出不应因净化失败而丢内容）。
fn sanitize_manifest_args(raw: &[u8], pkg_mcp_dir: &Path) -> Vec<u8> {
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(raw) else {
        return raw.to_vec();
    };
    let Some(args) = value.get_mut("args").and_then(|a| a.as_array_mut()) else {
        return raw.to_vec();
    };
    let mut changed = false;
    for arg in args.iter_mut() {
        let Some(s) = arg.as_str() else { continue };
        let path = Path::new(s);
        if !path.is_absolute() {
            continue;
        }
        let Ok(rel) = path.strip_prefix(pkg_mcp_dir) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        if rel.is_empty() || rel.starts_with("..") {
            continue;
        }
        *arg = serde_json::Value::String(rel);
        changed = true;
    }
    if !changed {
        return raw.to_vec();
    }
    serde_json::to_vec_pretty(&value).unwrap_or_else(|_| raw.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把 PINVOU3_HOME 指到干净临时目录跑闭包，借 ENV_LOCK 与其它 env 测试串行。
    fn with_temp_home<F: FnOnce()>(f: F) {
        let _g = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-pkgexport-test-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("PINVOU3_HOME", &dir);
        f();
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// args 净化（纯函数）：包内 mcp/ 绝对路径还原相对形式；相对参数与包外
    /// 绝对路径不动；非法 JSON 原样透传。路径用平台真实绝对路径构造（Windows 上
    /// `/foo` 非绝对路径——is_absolute 要求盘符/UNC）。
    #[test]
    fn sanitize_manifest_args_restores_only_in_package_abs_paths() {
        let base = std::env::temp_dir().join("pinvou3-pkgexport-sanitize");
        let mcp_dir = base.join("bundles/exp-mcp/mcp");
        let abs_entry = mcp_dir
            .join("server.py")
            .to_string_lossy()
            .replace('\\', "\\\\");
        let abs_outside = base
            .join("other/tool.py")
            .to_string_lossy()
            .replace('\\', "\\\\");
        let raw =
            format!(r#"{{"id":"exp-mcp","args":["{abs_entry}","--flag","{abs_outside}"]}}"#);
        let out = sanitize_manifest_args(raw.as_bytes(), &mcp_dir);
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        let outside_expected = base.join("other/tool.py").to_string_lossy().to_string();
        assert_eq!(
            value["args"],
            serde_json::json!(["server.py", "--flag", outside_expected]),
            "仅包内绝对路径还原，其余不动"
        );
        // 无改写需求 → 字节原样返回
        let clean = br#"{"id":"x","args":["server.py"]}"#;
        assert_eq!(sanitize_manifest_args(clean, &mcp_dir), clean);
        // 非法 JSON → 原样透传
        let broken = b"not-json{{{";
        assert_eq!(sanitize_manifest_args(broken, &mcp_dir), broken);
    }

    /// 已安装包导出：zip 内容完整（plugin.json/mcp/skills 平铺在根），
    /// manifest args 的包内绝对路径已还原为相对形式。
    #[test]
    fn export_installed_plugin_writes_sanitized_zip() {
        with_temp_home(|| {
            let pkg = paths::bundles_root().join("exp-mcp");
            let mcp_dir = pkg.join("mcp");
            std::fs::create_dir_all(pkg.join("skills/exp-skill")).unwrap();
            std::fs::create_dir_all(&mcp_dir).unwrap();
            std::fs::write(mcp_dir.join("server.py"), b"print('hi')").unwrap();
            std::fs::write(
                pkg.join("skills/exp-skill/SKILL.md"),
                "---\nname: exp-skill\n---\n",
            )
            .unwrap();
            std::fs::write(
                pkg.join("plugin.json"),
                r#"{"manifest_version":1,"id":"exp-mcp","name":"Exp"}"#,
            )
            .unwrap();
            // 模拟落了绝对路径 args 的 manifest（旧版本/手改/迁移路径的兜底对象）
            let abs_entry = mcp_dir.join("server.py").to_string_lossy().to_string();
            std::fs::write(
                mcp_dir.join("manifest.json"),
                format!(
                    r#"{{"id":"exp-mcp","name":"Exp","description":"d","version":"1","icon":"","category":"c","mcp_tools":[],"command":"python","args":["{abs_entry}"]}}"#,
                    abs_entry = abs_entry.replace('\\', "\\\\")
                ),
            )
            .unwrap();

            let dest = paths::pinvou3_home().join("export.zip");
            export_installed_plugin("exp-mcp", &dest).unwrap();

            let archive_file = std::fs::File::open(&dest).unwrap();
            let mut archive = zip::ZipArchive::new(archive_file).unwrap();
            let mut names: Vec<String> = (0..archive.len())
                .map(|i| archive.by_index(i).unwrap().name().to_string())
                .collect();
            names.sort();
            assert_eq!(
                names,
                vec![
                    "mcp/manifest.json".to_string(),
                    "mcp/server.py".to_string(),
                    "plugin.json".to_string(),
                    "skills/exp-skill/SKILL.md".to_string(),
                ]
            );
            let mut manifest = String::new();
            std::io::Read::read_to_string(
                &mut archive.by_name("mcp/manifest.json").unwrap(),
                &mut manifest,
            )
            .unwrap();
            let value: serde_json::Value = serde_json::from_str(&manifest).unwrap();
            assert_eq!(
                value["args"],
                serde_json::json!(["server.py"]),
                "包内绝对路径应还原为相对入口脚本名"
            );
            assert!(
                !manifest.contains(".pinvou3"),
                "导出 manifest 不应残留本机绝对路径: {manifest}"
            );
            // 源文件不被净化修改（导出只读源）
            let on_disk = std::fs::read_to_string(mcp_dir.join("manifest.json")).unwrap();
            assert!(on_disk.contains(&abs_entry.replace('\\', "\\\\")));
        });
    }

    /// fail-closed：未安装（目录不存在）/ 非法 id 报错，不留导出文件。
    #[test]
    fn export_installed_plugin_rejects_unknown_and_unsafe_id() {
        with_temp_home(|| {
            let dest = paths::pinvou3_home().join("export.zip");
            assert!(export_installed_plugin("never-installed", &dest).is_err());
            assert!(export_installed_plugin("../etc", &dest).is_err());
            assert!(!dest.exists(), "拒绝导出不得到留文件");
        });
    }

    /// 导出的已安装包 zip 可经统一导入管线重新导入（组件识别 + 落盘 + 登记）。
    #[test]
    fn exported_installed_zip_reimports_via_plugin_pipeline() {
        with_temp_home(|| {
            let pkg = paths::bundles_root().join("exp-mcp");
            std::fs::create_dir_all(pkg.join("mcp")).unwrap();
            std::fs::write(pkg.join("mcp/server.py"), b"print('hi')").unwrap();
            std::fs::write(
                pkg.join("plugin.json"),
                r#"{
                    "manifest_version":1,"id":"exp-mcp","name":"exp-mcp",
                    "components":{"mcp_servers":[{"id":"exp-mcp","dir":"mcp"}]}
                }"#,
            )
            .unwrap();
            std::fs::write(
                pkg.join("mcp/manifest.json"),
                r#"{"id":"exp-mcp","name":"exp-mcp","description":"d","version":"1","icon":"","category":"c","mcp_tools":[],"command":"python","args":["server.py"]}"#,
            )
            .unwrap();

            let dest = paths::pinvou3_home().join("export.zip");
            export_installed_plugin("exp-mcp", &dest).unwrap();
            // 模拟「在别的机器导入」：导出后移除原包目录。
            std::fs::remove_dir_all(&pkg).unwrap();

            let report = crate::features::marketplace::plugin_import::import_plugin_package(
                &dest.to_string_lossy(),
                "exp-mcp.zip",
            )
            .expect("导出的 zip 应可经统一导入管线重新导入");
            assert_eq!(report.id, "exp-mcp");
            assert_eq!(
                report.kind,
                crate::features::marketplace::bundle::BundleKind::Mcp
            );
            assert!(pkg.join("mcp/manifest.json").is_file(), "重新导入应落盘");
            assert!(pkg.join("mcp/server.py").is_file());
            let record = crate::features::marketplace::store::BundleStore::new()
                .get("exp-mcp")
                .unwrap()
                .expect("重新导入应登记");
            assert!(matches!(
                record.source,
                crate::features::marketplace::store::BundleSource::Upload(_)
            ));
        });
    }
}
