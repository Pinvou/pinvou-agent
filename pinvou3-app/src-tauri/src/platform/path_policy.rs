use std::path::{Path, PathBuf};

/// Resolve an artifact path against its execution workspace without imposing
/// command-layer knowledge on feature code.
pub(crate) fn resolve_artifact_path_in_workspace(raw: &str, workspace: &Path) -> String {
    if Path::new(raw).is_absolute() {
        raw.to_string()
    } else {
        workspace.join(raw).to_string_lossy().into_owned()
    }
}

/// 凭据和系统敏感组件黑名单——上传/摄入路径不得包含这些组件。
///
/// Wave 3 收敛：原先 `validate_browsable_path`（file_ingest）只挡 5 个敏感
/// **目录**（.ssh/.gnupg/.aws/.docker/.kube），挡不住 `~/keys/id_rsa`、
/// `~/config/.env`、`~/config/credentials.json` 等非目录凭据文件。此黑名单
/// 与 `validate_user_path` 共享同一份，确保所有用户路径入口校验一致。
pub(crate) const BLOCKED_COMPONENTS: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".docker",
    ".kube",
    ".password-store",
    "id_rsa",
    "id_ed25519",
    "id_ecdsa",
    "id_dsa",
    "credentials.json",
    ".env",
];

/// 系统敏感前缀黑名单——Unix 系统文件/虚拟文件系统路径。
pub(crate) const BLOCKED_PREFIXES: &[&str] = &[
    "/etc/shadow",
    "/etc/gshadow",
    "/etc/sudoers",
    "/etc/ssh/",
    "/root/",
    "/var/log/auth",
    "/proc/",
    "/sys/",
];

/// 检查已规范化路径是否触及凭据组件或系统敏感前缀。
/// 供 `validate_user_path` 和 `validate_browsable_path` 共享。
pub(crate) fn check_sensitive_components(canonical: &Path) -> Result<(), String> {
    let canonical_text = canonical.to_string_lossy();

    for blocked in BLOCKED_COMPONENTS {
        if canonical
            .components()
            .any(|component| crate::platform::os::path_component_eq(component.as_os_str(), blocked))
        {
            return Err(format!(
                "path {} crosses sensitive component {}",
                canonical.display(),
                blocked
            ));
        }
    }

    for prefix in BLOCKED_PREFIXES {
        if canonical_text.starts_with(prefix) {
            return Err(format!(
                "path {} is in system-sensitive area",
                canonical.display()
            ));
        }
    }

    Ok(())
}

/// Validate a user-controlled path before a feature reads or opens it.
///
/// Pinvou3 is a local single-user application, so paths outside the home
/// directory remain valid. Credential locations and system-sensitive paths
/// are rejected to keep their contents out of an external model context.
pub(crate) fn validate_user_path(raw: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return Err(format!("path must be absolute: {raw}"));
    }

    let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    check_sensitive_components(&canonical)?;
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_artifact_against_workspace() {
        let workspace = Path::new("C:/work/session");
        assert_eq!(
            resolve_artifact_path_in_workspace("output/report.md", workspace),
            workspace
                .join("output/report.md")
                .to_string_lossy()
                .into_owned()
        );
    }

    #[test]
    fn rejects_relative_user_path() {
        assert!(validate_user_path("relative/file.txt")
            .unwrap_err()
            .contains("must be absolute"));
    }

    #[test]
    fn rejects_sensitive_component() {
        let path = if cfg!(windows) {
            r"C:\Users\tester\.ssh\id_rsa"
        } else {
            "/home/tester/.ssh/id_rsa"
        };
        assert!(validate_user_path(path)
            .unwrap_err()
            .contains("sensitive component"));
    }

    /// 全黑名单逐项命中：组件比较须经平台感知 `path_component_eq`（Wave 3
    /// 委托后逐字节比较会让 Windows 大写变体绕过）。从常量读取黑名单构造
    /// 路径，避免测试代码内嵌敏感文件名字面量。
    #[test]
    fn rejects_all_blocked_components() {
        for blocked in BLOCKED_COMPONENTS {
            let path = std::env::temp_dir().join(blocked).join("x.txt");
            let r = check_sensitive_components(&path);
            assert!(r.is_err(), "{blocked} 组件应被拦");
        }
    }

    /// 大小写变体语义跟随平台：Windows 文件系统大小写不敏感，黑名单大写
    /// 写法同样必须命中；Unix 大小写敏感，大写变体是不同文件，不拦属正确
    /// 语义。此用例同时锁住「比较经由平台感知 helper」这一修复点。
    #[test]
    fn case_variant_semantics_follow_platform() {
        for blocked in BLOCKED_COMPONENTS {
            let upper = blocked.to_uppercase();
            let path = std::env::temp_dir().join(&upper).join("x.txt");
            let r = check_sensitive_components(&path);
            if cfg!(windows) {
                assert!(r.is_err(), "Windows 上 {upper} 应被拦");
            } else {
                assert!(r.is_ok(), "Unix 上 {upper} 是不同文件，不应拦");
            }
        }
    }
}
