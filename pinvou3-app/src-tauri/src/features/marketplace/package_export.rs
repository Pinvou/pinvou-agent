//! 包目录 → 标准插件包 zip 的导出：回收站导出与已安装包导出共用的写出逻辑。
//!
// architecture-guard: allow-target-cfg -- persist_tempfile 的 rename 覆盖需区分 Windows 瞬时占用重试与 Unix 直接 rename，属单函数 OS 原语差异；原子写失败场景由 cfg(unix) 测试覆盖，Windows 变体照搬 write_atomic 既有口径。
//!
//! zip 布局对齐 plugin-package-spec：包内容（plugin.json、mcp/、skills/ 等）
//! 平铺在 zip 根，可经统一导入管线 `plugin_import::import_plugin_package` 重新
//! 导入。已安装包导出拒绝预置目录 id：导入管线会拒绝与 `mcp_catalog` 预置 id
//! 冲突的包（防上传包顶替预置），预置导出的 zip 在任何机器都导不回去，fail-fast
//! 报错（见 `export_installed_plugin`）。只打包插件包本体（不含回收站清单等外部
//! 元数据）；Python 运行缓存
//! （`__pycache__/`、`*.pyc`）不打包（与导入管线的磁盘比对豁免口径一致）；
//! 符号链接不跟随、不打包（防把包外文件带进 zip / 链接环）。写出为原子写：
//! 同目录临时文件 + rename 覆盖，中途失败不动用户已确认覆盖的原目标文件。
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

/// 已安装包导出为标准插件包 zip。fail-closed：id 安全校验 + 预置目录包拒绝
/// （预置可从市场重装，导出必产出无法重导的死档案）+ 只导出 `bundles_root()`
/// 下真实存在的包目录（不存在的 id 报错，不导出任何文件）。并发契约：校验后
/// 全程持同 id `import_lock_for`（与导入/卸载/展示编辑同一把锁）——并发卸载/
/// 回收会把 `bundles/<id>` 搬走，锁内导出避免遍历到半搬移的目录却产出「成功」
/// 的不完整 zip（回收站导出由其自身 `file_lock()` 串行化，同口径）。
pub fn export_installed_plugin(pkg_id: &str, dest_zip: &Path) -> Result<(), String> {
    if !super::skill_marketplace::is_safe_skill_name(pkg_id) {
        return Err(format!("非法包 id '{pkg_id}'"));
    }
    // 预置目录包不导出：预置 id 受导入管线冲突保护（`plugin_import` 拒绝与
    // `mcp_catalog` 冲突的包），导出的 zip 无法重新导入；预置可从市场重新安装，
    // 导出无重导价值。迁移登记为 Preset 的手写自定义 MCP 不在目录内，不受此限。
    if crate::features::marketplace::mcp_catalog::spec_for(pkg_id).is_some() {
        return Err(format!(
            "包 '{pkg_id}' 属于市场预置，可从市场重新安装，无需导出（预置 id 受导入管线冲突保护，导出的 zip 无法重新导入）"
        ));
    }
    let import_lock = super::plugin_import::import_lock_for(pkg_id);
    let _import_guard = import_lock.lock().unwrap_or_else(|p| p.into_inner());
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
/// 原子写（底座 `write_atomic` 同范式，流式版）：先写目标同目录的临时文件，
/// 全部写完 sync 后 rename 覆盖目标——用户已确认覆盖的原文件在中途失败时
/// 保持原样不动；临时文件随 NamedTempFile drop 自动清理，不留半写 zip。
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
    // 同目录临时文件：rename 保证同盘原子替换；中途任何失败都不动原目标。
    let parent = dest_zip
        .parent()
        .ok_or_else(|| format!("目标 {} 无父目录，无法导出", dest_zip.display()))?;
    let tmp = tempfile::Builder::new()
        .prefix(".pinvou3-export-")
        .tempfile_in(parent)
        .map_err(|e| format!("创建临时文件（{} 所在目录）失败: {e}", dest_zip.display()))?;
    let mut zw = zip::ZipWriter::new(tmp);
    let opts = zip::write::SimpleFileOptions::default();
    for (rel, path) in &files {
        // 条目名即包内相对路径（盘上真实遍历产出，恒为 root 子孙，无穿越
        // 风险）；统一 '/' 分隔，与导入管线的路径口径一致。
        zw.start_file(rel, opts)
            .map_err(|e| format!("写 zip 条目 {rel}: {e}"))?;
        if sanitize_args && rel == "mcp/manifest.json" {
            let raw =
                std::fs::read(path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
            let sanitized = sanitize_manifest_args(&raw, &src_dir.join("mcp"));
            zw.write_all(&sanitized)
                .map_err(|e| format!("写 zip 条目 {rel}: {e}"))?;
        } else {
            let mut reader = std::fs::File::open(path)
                .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
            std::io::copy(&mut reader, &mut zw).map_err(|e| format!("写 zip 条目 {rel}: {e}"))?;
        }
    }
    let tmp = zw.finish().map_err(|e| format!("完成 zip 写入: {e}"))?;
    tmp.as_file()
        .sync_all()
        .map_err(|e| format!("同步临时文件失败: {e}"))?;
    persist_tempfile(tmp, dest_zip)?;
    Ok(files.len())
}

/// 临时文件 rename 覆盖落盘：Windows 上杀软/索引器可能短暂持有目标句柄导致
/// 替换被拒（底座 `write_atomic`、`rename_dir_with_retry` 同口径），对瞬时
/// 错误退避重试；永久性错误直接报错。失败时临时文件随 drop 清理，不动原目标。
#[cfg(not(windows))]
fn persist_tempfile(tmp: tempfile::NamedTempFile, dest_zip: &Path) -> Result<(), String> {
    tmp.persist(dest_zip)
        .map_err(|e| format!("落盘 {} 失败: {}", dest_zip.display(), e.error))?;
    Ok(())
}

/// Windows 变体：见非 Windows 版注释。
#[cfg(windows)]
fn persist_tempfile(tmp: tempfile::NamedTempFile, dest_zip: &Path) -> Result<(), String> {
    const MAX_PERSIST_ATTEMPTS: usize = 6;
    let mut pending = tmp;
    for attempt in 0..MAX_PERSIST_ATTEMPTS {
        match pending.persist(dest_zip) {
            Ok(_) => return Ok(()),
            Err(err) => {
                let retryable = err.error.kind() == std::io::ErrorKind::PermissionDenied
                    || matches!(err.error.raw_os_error(), Some(5 | 32 | 33));
                if !retryable || attempt + 1 == MAX_PERSIST_ATTEMPTS {
                    return Err(format!("落盘 {} 失败: {}", dest_zip.display(), err.error));
                }
                pending = err.file;
                std::thread::sleep(std::time::Duration::from_millis(
                    10u64.saturating_mul(1u64 << attempt),
                ));
            }
        }
    }
    Ok(())
}

/// 递归收集导出条目（相对路径用 '/' 分隔）：跳过 Python 运行缓存
/// （`__pycache__/` 子树与 `*.pyc`，与 plugin_import 的磁盘比对豁免口径一致）。
/// 符号链接一律跳过（`symlink_metadata` 不跟随）：用户手工放进包目录的链接
/// 若跟随会把包外文件打进 zip，链接环也无深度保护。
fn collect_export_files(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(String, PathBuf)>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
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
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
        f();
        match prev {
            Some(v) => unsafe { std::env::set_var("PINVOU3_HOME", v) },
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
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
        let raw = format!(r#"{{"id":"exp-mcp","args":["{abs_entry}","--flag","{abs_outside}"]}}"#);
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

    /// 预置目录包不导出：预置 id 受导入管线冲突保护，导出的 zip 在任何机器都
    /// 无法重新导入，导出必产出死档案——fail-fast 报错，不留导出文件。迁移
    /// 登记为 Preset 的手写自定义 MCP 不在 `mcp_catalog` 内，不受此限。
    #[test]
    fn export_installed_plugin_rejects_preset_catalog_id() {
        with_temp_home(|| {
            let preset_id = crate::features::marketplace::mcp_catalog::MCP_PACKAGES[0].id;
            let pkg = paths::bundles_root().join(preset_id);
            std::fs::create_dir_all(&pkg).unwrap();
            std::fs::write(pkg.join("plugin.json"), "{}").unwrap();
            let dest = paths::pinvou3_home().join("export.zip");
            let err = export_installed_plugin(preset_id, &dest).unwrap_err();
            assert!(err.contains("市场预置"), "错误应说明预置包不可导出: {err}");
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

    /// 符号链接不跟随：包目录里的链接文件/链接目录/链接环都不进 zip，
    /// 导出正常完成，只含真实文件（symlink 创建需 unix 权限语义，Windows
    /// 需开发者模式/特权，故仅 unix 跑）。
    #[cfg(unix)]
    #[test]
    fn write_package_zip_skips_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = std::env::temp_dir().join(format!(
            "pinvou3-pkgexport-symlink-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let pkg = dir.join("pkg");
        let outside = dir.join("outside");
        std::fs::create_dir_all(pkg.join("skills/s")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(pkg.join("plugin.json"), "{}").unwrap();
        std::fs::write(pkg.join("skills/s/SKILL.md"), "---\nname: s\n---\n").unwrap();
        std::fs::write(outside.join("secret.txt"), "包外内容不得入包").unwrap();
        // 链接文件（指向包外）、链接目录（指向包外）、链接环（指回祖先目录）
        symlink(outside.join("secret.txt"), pkg.join("linked-secret.txt")).unwrap();
        symlink(&outside, pkg.join("linked-dir")).unwrap();
        symlink(&pkg, pkg.join("skills/loop")).unwrap();

        let dest = dir.join("export.zip");
        let written = write_package_zip(&pkg, &dest, false).expect("含符号链接不应导致导出失败");
        assert_eq!(written, 2, "只有真实文件入包");

        let archive_file = std::fs::File::open(&dest).unwrap();
        let mut archive = zip::ZipArchive::new(archive_file).unwrap();
        let mut names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec!["plugin.json".to_string(), "skills/s/SKILL.md".to_string(),],
            "符号链接（文件/目录/环）一律不打包"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 原子写：中途失败（包内文件读到一半不可读）时，用户已确认覆盖的原目标
    /// 文件内容保持原样，且目标目录不留临时文件残骸。
    /// （chmod 000 注入读取失败依赖 unix 权限语义，故仅 unix 跑。）
    #[cfg(unix)]
    #[test]
    fn failed_export_keeps_existing_dest_untouched() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "pinvou3-pkgexport-atomic-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let pkg = dir.join("pkg");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("good.txt"), "ok").unwrap();
        // 排序后 good.txt 先写、z-broken.bin 后读 → 真实的「写了一半失败」。
        let broken = pkg.join("z-broken.bin");
        std::fs::write(&broken, "x").unwrap();
        std::fs::set_permissions(&broken, std::fs::Permissions::from_mode(0o000)).unwrap();

        let dest = dir.join("export.zip");
        std::fs::write(&dest, "用户已有的原文件内容").unwrap();

        assert!(
            write_package_zip(&pkg, &dest, false).is_err(),
            "读到不可读文件应失败"
        );
        assert_eq!(
            std::fs::read_to_string(&dest).unwrap(),
            "用户已有的原文件内容",
            "中途失败不得改动原目标文件"
        );
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with(".pinvou3-export-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "失败应清理临时文件，不留残骸: {leftovers:?}"
        );

        // 恢复权限，避免临时目录清理留下不可读文件。
        let _ = std::fs::set_permissions(&broken, std::fs::Permissions::from_mode(0o644));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
