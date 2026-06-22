//! 默认排除规则（见 docs/本地知识底座-产品形态与架构.md §4.1）。
//!
//! 两类目的：
//! 1. **隐私**——密钥/证书/token/浏览器 profile 连 L0 元数据都不收。
//! 2. **churn**——node_modules/.cache/.git/target 等高频变动目录，撑爆 inotify、灌满索引。
//!
//! 实现按**basename** 判定即可：扫描走 `walkdir::filter_entry`，被排除的目录会整株剪掉，
//! 不会再下探，所以无需对每个文件回溯全部祖先组件。

use std::collections::HashSet;
use std::path::{Component, Path};

/// basename 命中即排除（目录则整株剪枝）。
const SKIP_NAMES: &[&str] = &[
    // VCS
    ".git", ".svn", ".hg",
    // 依赖/构建产物
    "node_modules", "bower_components", "target", "dist", "build",
    "__pycache__", ".mypy_cache", ".pytest_cache", "venv", ".venv",
    ".cargo", ".rustup", ".npm", ".pnpm-store", ".yarn", ".gradle", ".m2",
    // 缓存/隐私配置/浏览器
    ".cache", ".config", ".mozilla", ".thumbnails",
    // 密钥
    ".ssh", ".gnupg",
    // 系统/包/回收站
    "snap", ".Trash-1000", "lost+found",
    // pinvou3 自身数据（避免索引自己的 session/DB churn）
    ".pinvou3", ".deepseek",
];

/// 扩展名命中即排除（小写，无点）。密钥证书 + 虚拟机镜像。
const SKIP_EXTS: &[&str] = &["key", "pem", "p12", "pfx", "vmdk", "qcow2", "vdi", "ova"];

/// 精确文件名命中即排除（散落在项目目录里的敏感文件）。
const SKIP_SECRET_FILES: &[&str] = &[
    "id_rsa", "id_dsa", "id_ecdsa", "id_ed25519",
    ".env", ".netrc", ".npmrc", "credentials", "known_hosts", "authorized_keys",
];

pub struct Excluder {
    names: HashSet<&'static str>,
    exts: HashSet<&'static str>,
    secrets: HashSet<&'static str>,
}

impl Default for Excluder {
    fn default() -> Self {
        Self {
            names: SKIP_NAMES.iter().copied().collect(),
            exts: SKIP_EXTS.iter().copied().collect(),
            secrets: SKIP_SECRET_FILES.iter().copied().collect(),
        }
    }
}

impl Excluder {
    /// `true` = 该 entry 应被排除。`ext` 传小写无点扩展名（目录传 None）。
    /// 用于 walkdir `filter_entry`：祖先已被剪枝，只需看 basename。
    pub fn is_skipped(&self, name: &str, _is_dir: bool, ext: Option<&str>) -> bool {
        if name.is_empty() {
            return true;
        }
        if self.names.contains(name) || self.secrets.contains(name) {
            return true;
        }
        if let Some(e) = ext {
            if self.exts.contains(e) {
                return true;
            }
        }
        false
    }

    /// 全路径排除：watcher 拿到的是任意路径（recursive 监听不会自动剪枝），
    /// 需检查**每一级**组件名 + 末级扩展名是否命中排除集。
    pub fn is_excluded_path(&self, path: &Path) -> bool {
        for comp in path.components() {
            if let Component::Normal(os) = comp {
                if let Some(name) = os.to_str() {
                    if self.names.contains(name) || self.secrets.contains(name) {
                        return true;
                    }
                }
            }
        }
        if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            if self.exts.contains(ext.to_lowercase().as_str()) {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_churn_and_secrets() {
        let ex = Excluder::default();
        assert!(ex.is_skipped("node_modules", true, None));
        assert!(ex.is_skipped(".ssh", true, None));
        assert!(ex.is_skipped(".cache", true, None));
        assert!(ex.is_skipped("id_rsa", false, None));
        assert!(ex.is_skipped(".env", false, None));
        assert!(ex.is_skipped("server.key", false, Some("key")));
        assert!(ex.is_skipped("disk.qcow2", false, Some("qcow2")));
    }

    #[test]
    fn keeps_normal_files_and_dirs() {
        let ex = Excluder::default();
        assert!(!ex.is_skipped("Documents", true, None));
        assert!(!ex.is_skipped("保险报价单.pdf", false, Some("pdf")));
        assert!(!ex.is_skipped("report.docx", false, Some("docx")));
        assert!(!ex.is_skipped("notes.md", false, Some("md")));
    }

    #[test]
    fn excluded_path_checks_all_components() {
        let ex = Excluder::default();
        assert!(ex.is_excluded_path(Path::new("/home/u/proj/node_modules/a/b/index.js")));
        assert!(ex.is_excluded_path(Path::new("/home/u/.ssh/config")));
        assert!(ex.is_excluded_path(Path::new("/home/u/proj/.env")));
        assert!(ex.is_excluded_path(Path::new("/home/u/vm/disk.qcow2")));
        assert!(!ex.is_excluded_path(Path::new("/home/u/Documents/保险报价单.pdf")));
        assert!(!ex.is_excluded_path(Path::new("/home/u/Downloads/report.docx")));
    }
}
