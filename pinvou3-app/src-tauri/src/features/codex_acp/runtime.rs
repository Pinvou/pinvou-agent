use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use wait_timeout::ChildExt;

/// codex-acp 1.1.5 官方依赖的最低 Codex CLI 版本。
/// 所有运行时来源都必须满足，显式覆盖路径也不能绕过兼容性门禁。
pub const MIN_CODEX_VERSION: &str = "0.144.6";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexRuntimeSource {
    Override,
    System,
    LegacyBundled,
}

impl CodexRuntimeSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Override => "override",
            Self::System => "system",
            Self::LegacyBundled => "legacy_bundled",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedCodex {
    pub path: PathBuf,
    /// 用于轻量自检与登录状态检查的原生可执行文件。npm shim 仍保留在 `path`
    /// 供 ACP adapter 启动，以维持包管理器环境；状态探测则绕过 `.cmd -> Node`
    /// 冷启动链路，避免把已安装 CLI 误判成缺失。
    pub probe_path: PathBuf,
    pub source: CodexRuntimeSource,
    pub version: String,
}

#[derive(Debug)]
struct NpmCodexInstall {
    version: String,
    native_path: PathBuf,
}

/// 一次候选探测同时返回可用运行时和系统 CLI 是否过旧，避免为了两个状态字段
/// 连续执行两次 `codex --version`。
#[derive(Debug, Clone)]
pub struct CodexRuntimeCandidates {
    pub resolved: Option<ResolvedCodex>,
    pub system_codex_incompatible: bool,
}

pub fn probe_codex_runtime(
    system_codex: Option<PathBuf>,
    legacy_bundled: Option<PathBuf>,
) -> CodexRuntimeCandidates {
    let override_candidate = std::env::var_os("PINVOU3_CODEX_PATH")
        .map(PathBuf::from)
        .and_then(|path| probe_codex(path, CodexRuntimeSource::Override));
    let system_candidate =
        system_codex.and_then(|path| probe_codex(path, CodexRuntimeSource::System));
    let system_codex_incompatible = system_candidate
        .as_ref()
        .is_some_and(|resolved| !runtime_version_is_compatible(&resolved.version));

    let resolved_override =
        override_candidate.filter(|candidate| runtime_version_is_compatible(&candidate.version));
    let resolved = match resolved_override {
        Some(resolved) => Some(resolved),
        None => {
            let legacy_candidate = legacy_bundled
                .and_then(|path| probe_codex(path, CodexRuntimeSource::LegacyBundled));
            select_newest_eligible([legacy_candidate, system_candidate])
        }
    };

    CodexRuntimeCandidates {
        resolved,
        system_codex_incompatible,
    }
}

pub fn resolve_codex_path(
    system_codex: Option<PathBuf>,
    legacy_bundled: Option<PathBuf>,
) -> Option<ResolvedCodex> {
    probe_codex_runtime(system_codex, legacy_bundled).resolved
}

fn probe_codex(path: PathBuf, source: CodexRuntimeSource) -> Option<ResolvedCodex> {
    if !path.is_file() {
        return None;
    }
    let npm_install = resolve_npm_codex_install(&path);
    let (probe_path, version) = match npm_install {
        Some(install) => (install.native_path, install.version),
        None => (path.clone(), codex_version(&path)?),
    };
    Some(ResolvedCodex {
        path,
        probe_path,
        source,
        version,
    })
}

/// npm 的全局 shim 会先启动 Node，再由 JS wrapper 启动数百 MB 的原生 Codex。
/// Windows 首次经过安全软件扫描时可能超过自检超时。这里仅在包清单与当前平台
/// 原生二进制同时完整存在时采用本地元数据；任一文件缺失都会回退真实 CLI 自检，
/// 因而不会把残缺安装误报为可用。
fn resolve_npm_codex_install(entrypoint: &Path) -> Option<NpmCodexInstall> {
    let (platform_package, target_triple, binary) = codex_native_target()?;
    npm_codex_package_roots(entrypoint)
        .into_iter()
        .find_map(|package_root| {
            let manifest_path = package_root.join("package.json");
            let manifest: serde_json::Value =
                serde_json::from_slice(&std::fs::read(manifest_path).ok()?).ok()?;
            if manifest.get("name").and_then(serde_json::Value::as_str) != Some("@openai/codex") {
                return None;
            }
            let version = manifest
                .get("version")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())?
                .to_string();
            let packaged = package_root
                .join("node_modules")
                .join("@openai")
                .join(platform_package)
                .join("vendor")
                .join(target_triple)
                .join("bin")
                .join(binary);
            let fallback = package_root
                .join("vendor")
                .join(target_triple)
                .join("bin")
                .join(binary);
            let native_path = [packaged, fallback].into_iter().find(|candidate| {
                candidate
                    .metadata()
                    .is_ok_and(|meta| meta.is_file() && meta.len() > 0)
            })?;
            Some(NpmCodexInstall {
                version,
                native_path,
            })
        })
}

fn npm_codex_package_roots(entrypoint: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(parent) = entrypoint.parent() {
        // Windows npm global prefix: <prefix>/codex.cmd + <prefix>/node_modules/...
        roots.push(parent.join("node_modules").join("@openai").join("codex"));
        // npm package-local .bin shim: node_modules/.bin/codex -> node_modules/@openai/codex
        if parent.file_name().and_then(|value| value.to_str()) == Some(".bin") {
            if let Some(node_modules) = parent.parent() {
                roots.push(node_modules.join("@openai").join("codex"));
            }
        }
        // Unix npm global prefix: <prefix>/bin/codex + <prefix>/lib/node_modules/...
        if let Some(prefix) = parent.parent() {
            roots.push(
                prefix
                    .join("lib")
                    .join("node_modules")
                    .join("@openai")
                    .join("codex"),
            );
        }
    }
    // Unix shims are commonly symlinks to @openai/codex/bin/codex.js.
    if let Ok(canonical) = entrypoint.canonicalize() {
        if canonical.file_name().and_then(|value| value.to_str()) == Some("codex.js") {
            if let Some(package_root) = canonical.parent().and_then(Path::parent) {
                roots.push(package_root.to_path_buf());
            }
        }
    }
    roots.sort();
    roots.dedup();
    roots
}

fn codex_native_target() -> Option<(&'static str, &'static str, &'static str)> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some(("codex-win32-x64", "x86_64-pc-windows-msvc", "codex.exe")),
        ("windows", "aarch64") => {
            Some(("codex-win32-arm64", "aarch64-pc-windows-msvc", "codex.exe"))
        }
        ("macos", "x86_64") => Some(("codex-darwin-x64", "x86_64-apple-darwin", "codex")),
        ("macos", "aarch64") => Some(("codex-darwin-arm64", "aarch64-apple-darwin", "codex")),
        ("linux", "x86_64") => Some(("codex-linux-x64", "x86_64-unknown-linux-musl", "codex")),
        ("linux", "aarch64") => Some(("codex-linux-arm64", "aarch64-unknown-linux-musl", "codex")),
        _ => None,
    }
}

/// 在满足最低兼容版本的候选中选版本最新者。
fn select_newest_eligible<const N: usize>(
    candidates: [Option<ResolvedCodex>; N],
) -> Option<ResolvedCodex> {
    candidates
        .into_iter()
        .flatten()
        .filter(|candidate| runtime_version_is_compatible(&candidate.version))
        .max_by(|left, right| compare_versions(&left.version, &right.version))
}

fn runtime_version_is_compatible(version: &str) -> bool {
    version_at_least(version, MIN_CODEX_VERSION)
}

/// 版本字符串是否不低于指定最低版本。
pub fn version_at_least(version: &str, minimum: &str) -> bool {
    compare_versions(version, minimum).is_ge()
}

/// 探测系统 PATH 或官方安装目录中的 codex 存在但版本低于 MIN_CODEX_VERSION 的情况，
/// 供 status 上报 system_codex_incompatible（区分「未安装」与「版本过低」）。
pub fn system_codex_incompatible(system_codex: Option<PathBuf>) -> bool {
    system_codex
        .and_then(|path| probe_codex(path, CodexRuntimeSource::System))
        .is_some_and(|resolved| !runtime_version_is_compatible(&resolved.version))
}

fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    parse_version(left).cmp(&parse_version(right))
}

fn parse_version(version: &str) -> Vec<u64> {
    version
        .split(['.', '-', '+'])
        .take_while(|part| part.chars().all(|character| character.is_ascii_digit()))
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

pub fn codex_version(path: &Path) -> Option<String> {
    // npm 安装的快路径：shim 旁边的 node_modules/@openai/codex/package.json
    // 直接读版本，免去 Node 冷启动（~9s）；布局不符或半成品安装时回退
    // spawn `codex --version`（慢但权威）。
    npm_codex_package_version(path).or_else(|| codex_version_result(path).ok())
}

/// npm shim 布局：`<prefix>/codex(.cmd)` + `<prefix>/node_modules/@openai/codex`。
/// 返回 package.json 的 version；要求 vendor 平台包目录存在（npm EBUSY 中断
/// 留下的半成品安装不算已安装）。
fn npm_codex_package_version(shim_path: &Path) -> Option<String> {
    let prefix = shim_path.parent()?;
    let package_dir = prefix.join("node_modules").join("@openai").join("codex");
    let raw = std::fs::read_to_string(package_dir.join("package.json")).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let version = parsed.get("version")?.as_str()?;
    if version.trim().is_empty() {
        return None;
    }
    let vendor_scope = package_dir.join("node_modules").join("@openai");
    let has_vendor = std::fs::read_dir(vendor_scope).ok()?.any(|entry| {
        entry
            .ok()
            .is_some_and(|dir| dir.file_name().to_string_lossy().starts_with("codex-"))
    });
    has_vendor.then(|| version.trim().to_string())
}

fn codex_version_result(path: &Path) -> Result<String> {
    let mut command = crate::platform::process::external_command(path);
    command
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .with_context(|| format!("启动 Codex 自检失败: {}", path.display()))?;
    let status = match child
        // Node 版 CLI 冷启动实测约 9 秒；给安全软件首次扫描留足时间，
        // 避免把已经安装的 Codex 误判为不可用。
        .wait_timeout(Duration::from_secs(15))
        .context("等待 Codex 自检进程失败")?
    {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Codex 自检超过 15 秒");
        }
    };
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    if !status.success() {
        bail!("Codex 自检进程退出: {status}; stderr={}", stderr.trim());
    }
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .context("读取 Codex 自检标准输出失败")?
        .read_to_string(&mut stdout)
        .context("解析 Codex 自检标准输出失败")?;
    parse_codex_version_output(&stdout).context("Codex 自检未返回版本号")
}

/// 从 `codex --version` 标准输出提取版本号。
/// 输出形如 `codex-cli 0.146.0`（带包名前缀）或裸 semver `0.146.0`，
/// 取首个以纯数字段开头的空白分隔字段；找不到视为不合规。
fn parse_codex_version_output(stdout: &str) -> Option<String> {
    stdout
        .split_whitespace()
        .find(|token| {
            token
                .split(['.', '-', '+'])
                .next()
                .is_some_and(|head| !head.is_empty() && head.chars().all(|c| c.is_ascii_digit()))
        })
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::codex_acp::platform;

    #[test]
    fn npm_package_version_reads_package_json_and_requires_vendor() {
        let dir =
            std::env::temp_dir().join(format!("npm-codex-version-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let package_dir = dir.join("node_modules").join("@openai").join("codex");
        std::fs::create_dir_all(
            package_dir
                .join("node_modules")
                .join("@openai")
                .join("codex-win32-x64"),
        )
        .unwrap();
        std::fs::write(
            package_dir.join("package.json"),
            r#"{"name":"@openai/codex","version":"0.146.1"}"#,
        )
        .unwrap();
        let shim = dir.join("codex.cmd");
        std::fs::write(&shim, "rem shim").unwrap();
        assert_eq!(
            npm_codex_package_version(&shim),
            Some("0.146.1".to_string())
        );
        // vendor 平台包缺失（半成品安装）→ None（回退 spawn 路径）
        std::fs::remove_dir_all(package_dir.join("node_modules")).unwrap();
        assert_eq!(npm_codex_package_version(&shim), None);
        // package.json 缺失 → None
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(npm_codex_package_version(&shim), None);
    }

    #[test]
    fn runtime_source_names_are_stable() {
        assert_eq!(CodexRuntimeSource::System.as_str(), "system");
        assert_eq!(CodexRuntimeSource::Override.as_str(), "override");
    }

    #[test]
    fn min_codex_version_enforced_by_semver_order() {
        assert!(runtime_version_is_compatible("0.144.6"));
        assert!(runtime_version_is_compatible("0.145.0"));
        assert!(runtime_version_is_compatible("1.0.0"));
        assert!(!runtime_version_is_compatible("0.144.5"));
        assert!(!runtime_version_is_compatible("0.143.9"));
        assert!(!runtime_version_is_compatible("unknown"));
    }

    #[test]
    fn version_output_parses_prefixed_and_bare_formats() {
        assert_eq!(
            parse_codex_version_output("codex-cli 0.146.0"),
            Some("0.146.0".to_string())
        );
        assert_eq!(
            parse_codex_version_output("0.146.0"),
            Some("0.146.0".to_string())
        );
        assert_eq!(parse_codex_version_output("codex-cli"), None);
        assert_eq!(parse_codex_version_output(""), None);
        assert_eq!(parse_codex_version_output("not-a-version"), None);
    }

    #[test]
    fn every_runtime_source_rejects_incompatible_versions() {
        for source in [
            CodexRuntimeSource::Override,
            CodexRuntimeSource::System,
            CodexRuntimeSource::LegacyBundled,
        ] {
            let selected = select_newest_eligible([Some(ResolvedCodex {
                path: PathBuf::from(source.as_str()),
                probe_path: PathBuf::from(source.as_str()),
                source,
                version: "0.143.9".to_string(),
            })]);
            assert!(selected.is_none(), "{source:?} 不应绕过最低 Codex 版本门禁");
        }
    }

    fn fake_codex(version: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-codex-version-test-{}-{version}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create fake codex directory");
        let path = root.join("codex");
        std::fs::write(&path, format!("#!/bin/sh\necho \"codex {version}\"\n"))
            .expect("write fake codex");
        platform::make_executable(&path).expect("chmod fake codex");
        path
    }

    #[test]
    fn system_codex_below_min_version_is_rejected() {
        if !platform::unix_like() {
            return;
        }
        let outdated = fake_codex("0.100.0");
        let resolved = resolve_codex_path(Some(outdated.clone()), None);
        assert!(
            resolved
                .as_ref()
                .is_none_or(|resolved| resolved.source != CodexRuntimeSource::System),
            "低版本系统 codex 不应作为 System 来源入选"
        );
        assert!(system_codex_incompatible(Some(outdated.clone())));
        let _ = std::fs::remove_dir_all(outdated.parent().expect("fake codex parent"));
    }

    #[test]
    fn system_codex_at_min_version_is_accepted() {
        if !platform::unix_like() {
            return;
        }
        let current = fake_codex(MIN_CODEX_VERSION);
        let resolved = resolve_codex_path(Some(current.clone()), None);
        assert_eq!(
            resolved.map(|resolved| resolved.source),
            Some(CodexRuntimeSource::System)
        );
        assert!(!system_codex_incompatible(Some(current.clone())));
        let _ = std::fs::remove_dir_all(current.parent().expect("fake codex parent"));
    }

    #[test]
    fn newest_eligible_candidate_wins() {
        let selected = select_newest_eligible([
            Some(ResolvedCodex {
                path: PathBuf::from("legacy"),
                probe_path: PathBuf::from("legacy"),
                source: CodexRuntimeSource::LegacyBundled,
                version: MIN_CODEX_VERSION.to_string(),
            }),
            Some(ResolvedCodex {
                path: PathBuf::from("system"),
                probe_path: PathBuf::from("system"),
                source: CodexRuntimeSource::System,
                version: "0.145.0".to_string(),
            }),
        ])
        .unwrap();
        assert_eq!(selected.source, CodexRuntimeSource::System);
        assert_eq!(selected.version, "0.145.0");
    }

    #[test]
    fn npm_codex_install_uses_manifest_and_native_probe_binary() {
        let Some((platform_package, target_triple, binary)) = codex_native_target() else {
            return;
        };
        let root = std::env::temp_dir().join(format!(
            "pinvou3-codex-npm-layout-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let entrypoint = root.join("codex-shim");
        let package_root = root.join("node_modules").join("@openai").join("codex");
        let native = package_root
            .join("node_modules")
            .join("@openai")
            .join(platform_package)
            .join("vendor")
            .join(target_triple)
            .join("bin")
            .join(binary);
        std::fs::create_dir_all(native.parent().expect("native parent"))
            .expect("create fake npm layout");
        std::fs::write(&entrypoint, "shim").expect("write fake shim");
        std::fs::write(
            package_root.join("package.json"),
            r#"{"name":"@openai/codex","version":"0.146.0"}"#,
        )
        .expect("write fake package manifest");
        std::fs::write(&native, "native").expect("write fake native binary");

        let install = resolve_npm_codex_install(&entrypoint).expect("resolve npm Codex install");
        assert_eq!(install.version, "0.146.0");
        assert_eq!(install.native_path, native);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn npm_codex_install_rejects_missing_native_binary() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-codex-incomplete-npm-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let entrypoint = root.join("codex-shim");
        let package_root = root.join("node_modules").join("@openai").join("codex");
        std::fs::create_dir_all(&package_root).expect("create fake package root");
        std::fs::write(&entrypoint, "shim").expect("write fake shim");
        std::fs::write(
            package_root.join("package.json"),
            r#"{"name":"@openai/codex","version":"0.146.0"}"#,
        )
        .expect("write fake package manifest");

        assert!(resolve_npm_codex_install(&entrypoint).is_none());
        let _ = std::fs::remove_dir_all(root);
    }
}
