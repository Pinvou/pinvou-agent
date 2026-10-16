use std::path::{Path, PathBuf};

use codewhale_execpolicy::sensitive_paths;

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
///
/// 引擎清单单源（CodeWhale #87 `sensitive_paths` 模块）：本清单是引擎
/// `SENSITIVE_DIRECTORY_NAMES` ∪ `SENSITIVE_FILE_NAMES` ∪
/// `SENSITIVE_COMPONENT_NAMES` 中 Pinvou 摄入门所需子集的登记，下方 `const`
/// 断言把每项锚定在引擎清单上——引擎侧改名/删除会在编译期失败并强制同步，
/// 不再靠注释对齐。刻意的引擎残差（不登记）：
/// 多段目录名（`.config/gcloud` 等——本闸按「任意单个路径组件相等」比较，
/// 多段字面量永不命中；对应敏感面由引擎 ruleset 按 `~/` 相对拼写承担）、
/// `.azure`/`.dws`/`.tmeet` 目录、以及仅 home 根敏感的裸文件名
/// （`credentials`/`secrets`/`.pgp`/`.gpg`/`.netrc`/`.git-credentials`——
/// 组件面登记会误拦同名工作目录）。`id_rsa`/`id_ed25519`/`id_ecdsa`/`id_dsa`
/// 已迁移为引擎组件面派生（`ID_COMPONENT_KEYS`：#87 评审轮给引擎补上裸
/// `id_*` 组件面后，按常量上的预登记迁移点改为从
/// `SENSITIVE_COMPONENT_NAMES` 编译期锚定；Wave 3 动机即 `~/keys/id_rsa`）。
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
];

/// 系统敏感前缀黑名单——Unix 系统文件/虚拟文件系统路径。
///
/// 引擎清单单源：本清单 ⊆ 引擎 `SENSITIVE_ABSOLUTE_PREFIXES`（const 断言
/// 锚定）。引擎残差（`/etc/shadow-`、`/etc/shadow.bak`、`/etc/gshadow-`、
/// `/etc/sudoers-`、`/etc/sudoers.bak`、`/etc/sudoers.d/`、`/var/log/auth.log`）
/// 是编辑器/老化备份、碎片目录与轮变体的精确词面——引擎 ruleset 的 token 面
/// 需要逐一枚举，本闸 `starts_with` 语义下已被 `/etc/shadow`、`/etc/gshadow`、
/// `/etc/sudoers`、`/var/log/auth` 前缀覆盖，不重复登记。
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

// ── 引擎清单编译期锚定（与 features/assistant::safety_deny_rules 共用）──────
// const eval 里不能调用 PartialEq（非 const fn），一律按字节比较。

/// 逐字节 `&str` 相等（const eval 用）。
pub(crate) const fn inventory_str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// `inventory` 中是否含 `entry`（const eval 用）。
pub(crate) const fn inventory_contains(inventory: &[&str], entry: &str) -> bool {
    let mut i = 0;
    while i < inventory.len() {
        if inventory_str_eq(inventory[i], entry) {
            return true;
        }
        i += 1;
    }
    false
}

/// `inventory` 中是否存在恰为 `prefix`+`suffix` 拼接的条目（const eval 用；
/// 引擎按 `~/` 相对路径拼写登记文件，本应用按（名字，属主目录）二元组建档）。
pub(crate) const fn inventory_has_joined(inventory: &[&str], prefix: &str, suffix: &str) -> bool {
    let mut i = 0;
    while i < inventory.len() {
        let hay = inventory[i].as_bytes();
        let p = prefix.as_bytes();
        let s = suffix.as_bytes();
        if hay.len() == p.len() + s.len() {
            let mut matched = true;
            let mut k = 0;
            while k < p.len() {
                if hay[k] != p[k] {
                    matched = false;
                    break;
                }
                k += 1;
            }
            let mut k = 0;
            while matched && k < s.len() {
                if hay[p.len() + k] != s[k] {
                    matched = false;
                }
                k += 1;
            }
            if matched {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// 引擎组件面派生的裸 `id_*` 私钥名（预登记迁移已落地）：#87 评审轮给引擎
/// 补上 `SENSITIVE_COMPONENT_NAMES`（含裸 `id_*`）后，原「app 专属组件面 +
/// 互斥断言」按常量上的预登记迁移点退役——四项保留在 `BLOCKED_COMPONENTS`
/// 中，但每项编译期锚定在引擎组件面上：引擎侧改名/删除让本 crate 编译失败，
/// 强制有意识同步；引擎组件面新增项由 forkguard 覆盖测试强制采纳或登记残差。
/// Wave 3 动机不变：`~/keys/id_rsa` 这类任意目录下的密钥文件。
const ID_COMPONENT_KEYS: &[&str] = &["id_rsa", "id_ed25519", "id_ecdsa", "id_dsa"];

const fn all_components_in_engine_inventory(entries: &[&str]) -> bool {
    let mut i = 0;
    while i < entries.len() {
        if !inventory_contains(sensitive_paths::SENSITIVE_DIRECTORY_NAMES, entries[i])
            && !inventory_contains(sensitive_paths::SENSITIVE_FILE_NAMES, entries[i])
            && !inventory_contains(sensitive_paths::SENSITIVE_COMPONENT_NAMES, entries[i])
        {
            return false;
        }
        i += 1;
    }
    true
}

const fn all_entries_in(entries: &[&str], inventory: &[&str]) -> bool {
    let mut i = 0;
    while i < entries.len() {
        if !inventory_contains(inventory, entries[i]) {
            return false;
        }
        i += 1;
    }
    true
}

const fn all_in_engine_prefixes(entries: &[&str]) -> bool {
    let mut i = 0;
    while i < entries.len() {
        if !inventory_contains(sensitive_paths::SENSITIVE_ABSOLUTE_PREFIXES, entries[i]) {
            return false;
        }
        i += 1;
    }
    true
}

// 子集锚定：引擎清单任何一侧的改名/删除让本 crate 编译失败，强制有意识同步。
const _: () = assert!(
    all_components_in_engine_inventory(BLOCKED_COMPONENTS),
    "BLOCKED_COMPONENTS 必须逐项存在于引擎 sensitive_paths 三份清单（改名/删除需同步本子集）"
);
const _: () = assert!(
    all_entries_in(
        ID_COMPONENT_KEYS,
        sensitive_paths::SENSITIVE_COMPONENT_NAMES
    ) && all_entries_in(ID_COMPONENT_KEYS, BLOCKED_COMPONENTS),
    "裸 id_* 必须从引擎 SENSITIVE_COMPONENT_NAMES 派生且保留在 BLOCKED_COMPONENTS"
);
const _: () = assert!(
    all_in_engine_prefixes(BLOCKED_PREFIXES),
    "BLOCKED_PREFIXES 必须逐项存在于引擎 SENSITIVE_ABSOLUTE_PREFIXES"
);

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

    /// 引擎清单单源锚定（const 断言的运行期镜像 + 残差登记）：本闸两份黑名单
    /// 是引擎 `sensitive_paths` 三份清单的 Pinvou 子集，引擎侧任何改名/删除
    /// 同时被编译期断言与本用例拦截；残差（引擎有、Pinvou 不登记）按注释
    /// 登记的集合逐项钉死，改成任何一侧都要有意识。引擎四个面各有覆盖方向
    /// 循环：引擎新增条目必须被采纳或登记为残差，不再能静默漏过。
    #[test]
    fn forkguard_blocked_lists_are_the_anchored_subset_of_engine_inventory() {
        use sensitive_paths::{
            SENSITIVE_ABSOLUTE_PREFIXES, SENSITIVE_COMPONENT_NAMES, SENSITIVE_DIRECTORY_NAMES,
            SENSITIVE_FILE_NAMES,
        };

        let in_engine = |entry: &str| {
            SENSITIVE_DIRECTORY_NAMES.contains(&entry)
                || SENSITIVE_FILE_NAMES.contains(&entry)
                || SENSITIVE_COMPONENT_NAMES.contains(&entry)
        };

        // 子集方向（编译期断言的镜像）：每项都在引擎三份清单内。
        for entry in BLOCKED_COMPONENTS {
            assert!(in_engine(entry), "{entry} 应在引擎清单");
        }
        for entry in BLOCKED_PREFIXES {
            assert!(
                SENSITIVE_ABSOLUTE_PREFIXES.contains(&entry),
                "{entry} 应在引擎前缀清单"
            );
        }

        // 引擎组件面派生的裸 id_*（预登记迁移已落地，const 断言镜像）：保留
        // 在组件黑名单且逐项锚定在引擎组件面上。
        for key in ID_COMPONENT_KEYS {
            assert!(BLOCKED_COMPONENTS.contains(key), "{key} 应保留");
            assert!(
                SENSITIVE_COMPONENT_NAMES.contains(key),
                "{key} 应锚定在引擎组件面"
            );
        }

        // 刻意的引擎残差：目录面（多段/未采纳目录名——多段字面量在组件相等
        // 比较下永不命中）。
        let dir_residual = [
            ".gnupg/private-keys-v1.d",
            ".config/gcloud",
            ".azure",
            ".config/google-chrome",
            ".mozilla/firefox",
            ".dws",
            ".tmeet",
        ];
        // 文件面残差：`.ssh/*` 密钥/信任文件以路径拼写登记（组件面由 app 专属
        // `id_*` 与目录面覆盖）；仅 home 根敏感的裸文件名登记会误拦同名工作
        // 目录；`credentials.json`/`.env` 是引擎注释认可的任意组件敏感面，
        // Pinvou 已登记。
        let file_residual = [
            ".ssh/id_rsa",
            ".ssh/id_ed25519",
            ".ssh/id_ecdsa",
            ".ssh/id_dsa",
            ".ssh/authorized_keys",
            ".ssh/config",
            ".kube/config",
            ".docker/config.json",
            ".aws/config",
            ".aws/credentials",
            ".config/gcloud/application_default_credentials.json",
            ".config/gcloud/credentials.db",
            ".azure/msal_token_cache.json",
            ".azure/accessTokens.json",
            ".config/google-chrome/Default/Cookies",
            ".config/google-chrome/Default/Login Data",
            ".config/google-chrome/Local State",
            ".gnupg/secring.gpg",
            "credentials",
            "secrets",
            ".pgp",
            ".gpg",
            ".netrc",
            ".git-credentials",
        ];
        for residual in dir_residual.iter().chain(file_residual.iter()) {
            assert!(
                !BLOCKED_COMPONENTS.contains(&residual),
                "{residual} 若要登记进组件黑名单，须同步更新本残差登记与 const 断言"
            );
            assert!(
                in_engine(residual),
                "残差登记本身必须仍在引擎清单内: {residual}"
            );
        }

        // 组件面残差：`.dws`/`.tmeet` 引擎按「任意组件」登记（组件面清单），
        // 本闸不采纳——摄取前缀面无此二者、目录面残差同名登记（命令通道语料
        // 经 SENSITIVE_DIR_NAMES 别名承担）。引擎组件面若新增条目，覆盖循环
        // 会强制在此登记或采纳。
        let component_residual = [".dws", ".tmeet"];
        for residual in component_residual {
            assert!(
                !BLOCKED_COMPONENTS.contains(&residual),
                "{residual} 若要登记进组件黑名单，须同步更新本残差登记"
            );
            assert!(
                SENSITIVE_COMPONENT_NAMES.contains(&residual),
                "残差登记本身必须仍在引擎组件面内: {residual}"
            );
        }

        // 前缀面残差：备份/`-` 变体、sudoers.d 与轮变体已被 starts_with 语义
        // 覆盖（`/var/log/auth.log` 为 #87 评审轮新增，被采纳的
        // `/var/log/auth` 前缀覆盖）。
        for residual in [
            "/etc/shadow-",
            "/etc/shadow.bak",
            "/etc/gshadow-",
            "/etc/sudoers-",
            "/etc/sudoers.bak",
            "/etc/sudoers.d/",
            "/var/log/auth.log",
        ] {
            assert!(
                !BLOCKED_PREFIXES.contains(&residual),
                "{residual} 若要登记，须同步更新本残差登记"
            );
            assert!(SENSITIVE_ABSOLUTE_PREFIXES.contains(&residual));
        }

        // 引擎覆盖方向：每个引擎条目必须被采纳或登记为残差。仅靠子集断言时，
        // 引擎新增条目会静默漏过（正是单源化要暴露的漂移类）；本方向强制引
        // 擎新增在本表显式登记（或采纳），四个面各一。
        for engine_dir in SENSITIVE_DIRECTORY_NAMES {
            assert!(
                BLOCKED_COMPONENTS.contains(engine_dir) || dir_residual.contains(engine_dir),
                "引擎目录 {engine_dir} 未被采纳也不在残差登记中"
            );
        }
        for engine_file in SENSITIVE_FILE_NAMES {
            assert!(
                BLOCKED_COMPONENTS.contains(engine_file) || file_residual.contains(engine_file),
                "引擎文件 {engine_file} 未被采纳也不在残差登记中"
            );
        }
        for engine_component in SENSITIVE_COMPONENT_NAMES {
            assert!(
                BLOCKED_COMPONENTS.contains(engine_component)
                    || component_residual.contains(engine_component),
                "引擎组件 {engine_component} 未被采纳也不在残差登记中"
            );
        }
        for engine_prefix in SENSITIVE_ABSOLUTE_PREFIXES {
            let subsumed = BLOCKED_PREFIXES
                .iter()
                .any(|p| engine_prefix.starts_with(*p));
            assert!(
                BLOCKED_PREFIXES.contains(engine_prefix) || subsumed,
                "引擎前缀 {engine_prefix} 未被采纳也不被 starts_with 覆盖"
            );
        }
    }
}
