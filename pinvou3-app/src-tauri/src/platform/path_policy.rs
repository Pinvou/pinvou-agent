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
/// `pub` for the pinvou-cli `check_sensitive_path` mirror (artifacts.rs): the
/// CLI imports these lists instead of a hand copy, so an upstream credential
/// addition can never silently reopen the CLI surface — the drift the copy's
/// own doc warned about.
pub const BLOCKED_COMPONENTS: &[&str] = &[
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
    // 与 `.env` 同类的本地凭据/密钥文件,逐一枚举的黑名单此前漏掉的常見
    // 命名(direnv 的 .envrc 常含导出的密钥;.git-credentials/.netrc/
    // .pgpass/.npmrc 是各工具的默认凭据存储)。
    ".envrc",
    ".git-credentials",
    ".netrc",
    ".pgpass",
    ".npmrc",
];

/// 组件前缀黑名单:`.env.` 系命名(`.env.local`、`.env.production`、
/// `.env.development`……)与 `.env` 同为本地环境凭据文件,环境变体命名
/// 无法逐一枚举,按前缀判定。匹配经由平台感知的 `path_component_prefix`
/// (macOS/Windows ASCII 折叠,与 `path_component_eq` 同语义)。
pub const BLOCKED_COMPONENT_PREFIXES: &[&str] = &[".env."];

/// 系统敏感前缀黑名单——Unix 系统文件/虚拟文件系统路径。
pub const BLOCKED_PREFIXES: &[&str] = &[
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

    for prefix in BLOCKED_COMPONENT_PREFIXES {
        if canonical.components().any(|component| {
            crate::platform::os::path_component_prefix(component.as_os_str(), prefix)
        }) {
            return Err(format!(
                "path {} crosses sensitive component prefix {}",
                canonical.display(),
                prefix
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
        assert!(
            validate_user_path("relative/file.txt")
                .unwrap_err()
                .contains("must be absolute")
        );
    }

    #[test]
    fn rejects_sensitive_component() {
        let path = if cfg!(windows) {
            r"C:\Users\tester\.ssh\id_rsa"
        } else {
            "/home/tester/.ssh/id_rsa"
        };
        assert!(
            validate_user_path(path)
                .unwrap_err()
                .contains("sensitive component")
        );
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

    /// 大小写变体语义跟随平台：Windows 与 macOS 默认卷大小写不敏感，黑名单
    /// 大写写法同样必须命中（卷层会把 `.AWS` 解析到磁盘上的 `.aws`，按调用方
    /// 拼写字节比较的词法检查会被绕过，见 posix.rs `path_component_eq`）；
    /// Linux 大小写敏感，大写变体是不同文件，不拦属正确语义。此用例同时锁住
    /// 「比较经由平台感知 helper」这一修复点。
    #[test]
    fn case_variant_semantics_follow_platform() {
        for blocked in BLOCKED_COMPONENTS {
            let upper = blocked.to_uppercase();
            let path = std::env::temp_dir().join(&upper).join("x.txt");
            let r = check_sensitive_components(&path);
            if cfg!(windows) || cfg!(target_os = "macos") {
                assert!(r.is_err(), "{upper} 应被拦");
            } else {
                assert!(r.is_ok(), "Linux 上 {upper} 是不同文件，不应拦");
            }
        }
    }

    /// 大写变体逐项钉住比较语义：大小写不敏感平台上 `.ENV`/`.AWS` 等拼写
    /// 会解析到与原样拼写相同的磁盘文件，词法黑名单必须同样命中；Linux 上
    /// 同一批大写变体是不同文件、放行，而原样拼写仍拦截。
    /// check_sensitive_components 纯组件判定、不触盘，断言的是比较语义本身。
    /// 大小写不敏感卷上 canonicalize 输出的拼写是平台实现细节（macOS
    /// realpath 保留调用方拼写），任何判定都不得依赖它——词法与解析后两次
    /// 判定都经折叠比较，折叠是承载语义的判定，不得改回字节精确比较。
    #[test]
    fn uppercase_credential_variants_follow_platform() {
        let uppercase_variants = [
            ".ENV",
            ".AWS/credentials",
            ".KUBE/config",
            ".DOCKER/config.json",
            ".SSH",
        ];
        for raw in uppercase_variants {
            let path = std::env::temp_dir().join(raw);
            let r = check_sensitive_components(&path);
            if cfg!(windows) || cfg!(target_os = "macos") {
                assert!(r.is_err(), "大小写不敏感平台上 {raw} 应被拦");
            } else {
                assert!(r.is_ok(), "Linux 上 {raw} 是不同文件，不应拦");
            }
        }
        for raw in uppercase_variants {
            let lower = raw.to_lowercase();
            let path = std::env::temp_dir().join(&lower);
            assert!(
                check_sensitive_components(&path).is_err(),
                "原样拼写 {lower} 应被拦"
            );
        }
    }

    #[test]
    fn env_variants_and_credential_files_are_blocked() {
        // `.env.` 系环境变体逐一枚举追不全,按前缀拦截;.envrc 与各工具的
        // 默认凭据文件按精确组件拦截。
        let blocked = [
            ".env.local",
            ".env.production",
            ".env.development",
            ".envrc",
            ".git-credentials",
            ".netrc",
            ".pgpass",
            ".npmrc",
        ];
        for raw in blocked {
            let path = std::env::temp_dir().join("app").join(raw);
            assert!(check_sensitive_components(&path).is_err(), "{raw} 必须被拦");
        }
        // 前缀仅按组件边界生效:同名前缀的普通文件不受影响。
        for benign in ["environment.md", "envelope.txt", ".environment"] {
            let path = std::env::temp_dir().join("app").join(benign);
            assert!(
                check_sensitive_components(&path).is_ok(),
                "{benign} 不是凭据文件,不应被拦"
            );
        }
    }

    #[test]
    fn env_variant_case_semantics_follow_platform() {
        // 与 uppercase_credential_variants_follow_platform 同判据:.ENV.LOCAL
        // 这类大写变体在大小写不敏感卷(macOS/Windows)上解析到同一文件,
        // 词法前缀判定必须同样折叠;Linux 上是不同文件,不拦。
        let path = std::env::temp_dir().join("app").join(".ENV.LOCAL");
        if cfg!(windows) || cfg!(target_os = "macos") {
            assert!(check_sensitive_components(&path).is_err());
        } else {
            assert!(check_sensitive_components(&path).is_ok());
        }
    }
}
