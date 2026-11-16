//! 默认排除规则。
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
    ".git",
    ".svn",
    ".hg",
    // 依赖/构建产物
    "node_modules",
    "bower_components",
    "target",
    "dist",
    "build",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    "venv",
    ".venv",
    ".cargo",
    ".rustup",
    ".npm",
    ".pnpm-store",
    ".yarn",
    ".gradle",
    ".m2",
    // 缓存/隐私配置/浏览器
    ".cache",
    ".config",
    ".mozilla",
    ".thumbnails",
    // 密钥
    ".ssh",
    ".gnupg",
    // 系统/包/回收站
    "snap",
    ".Trash-1000",
    "lost+found",
    // pinvou3 自身数据（避免索引自己的 session/DB churn）
    ".pinvou3",
    ".deepseek",
];

/// 扩展名命中即排除（小写，无点）。密钥证书 + 虚拟机镜像。
const SKIP_EXTS: &[&str] = &["key", "pem", "p12", "pfx", "vmdk", "qcow2", "vdi", "ova"];

/// 精确文件名命中即排除（散落在项目目录里的敏感文件）。
const SKIP_SECRET_FILES: &[&str] = &[
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    ".env",
    ".netrc",
    ".npmrc",
    "credentials",
    "known_hosts",
    "authorized_keys",
];

/// 常用类型白名单(办公+媒体)：只索引这些扩展名的**文件**；目录不受影响照常下探。
/// 砍掉源码/编译产物/无扩展名等噪音——实测占库 94%(245 万→13.7 万)。
/// 换类型改这里(用户可配入口暂未做)。注意 keynote 的 `key` 与密钥 `.key` 冲突，故不收。
const ALLOW_EXTS: &[&str] = &[
    // 文档
    "doc", "docx", "pdf", "txt", "md", "markdown", "rtf", "odt", "wps", "pages", "tex", "html",
    "htm", "mhtml", "mht", // 表格
    "xls", "xlsx", "csv", "ods", "et", "numbers", // 演示
    "ppt", "pptx", "odp", "dps", // 图片
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "heic", "heif", "tiff", "tif", "ico",
    "avif", // 设计
    "psd", "ai", "sketch", "fig", "xd", "cdr", "eps", // 压缩
    "zip", "rar", "7z", "tar", "gz", "bz2", "xz", "tgz", // 音视频
    "mp3", "wav", "flac", "m4a", "aac", "ogg", "mp4", "mov", "avi", "mkv", "wmv", "webm", "flv",
    // 电子书
    "epub", "mobi", "azw3", "fb2",
];

pub struct Excluder {
    names: HashSet<&'static str>,
    exts: HashSet<&'static str>,
    secrets: HashSet<&'static str>,
    /// 仅索引这些扩展名的文件(白名单)。
    allow: HashSet<&'static str>,
}

impl Default for Excluder {
    fn default() -> Self {
        Self {
            names: SKIP_NAMES.iter().copied().collect(),
            exts: SKIP_EXTS.iter().copied().collect(),
            secrets: SKIP_SECRET_FILES.iter().copied().collect(),
            allow: ALLOW_EXTS.iter().copied().collect(),
        }
    }
}

impl Excluder {
    /// `true` = 该 entry 应被排除。`ext` 传小写无点扩展名（目录传 None）。
    /// 用于 walkdir `filter_entry`：祖先已被剪枝，只需看 basename。
    pub fn is_skipped(&self, name: &str, is_dir: bool, ext: Option<&str>) -> bool {
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
        // 类型白名单：目录照常下探(不判)；文件的扩展名不在白名单(含无扩展名)→ 排除。
        if !is_dir {
            match ext {
                Some(e) if self.allow.contains(e) => {}
                _ => return true,
            }
        }
        false
    }

    /// A single basename hitting names/secrets excludes (the minimal unit
    /// of component-level judgment).
    #[allow(dead_code)] // watcher 库先行：接线前仅测试与 watcher(未接线)引用
    pub fn is_excluded_name(&self, name: &str) -> bool {
        self.names.contains(name) || self.secrets.contains(name)
    }

    /// Component-level exclusion: the path is excluded when **any** level's
    /// basename (including the last) hits names/secrets. Used by the watcher
    /// — recursive watching does not prune automatically, so paths deep
    /// inside node_modules/.ssh etc. still surface in events, while the
    /// scanner prunes level by level through walkdir's `filter_entry` and
    /// has no such need.
    ///
    /// The extension policy is not judged here: it needs the `is_dir`
    /// context (directories are not subject to the block/allow lists,
    /// otherwise dot-suffixed directories like `backup.old` would be wrongly
    /// excluded); the caller stats and hands the entry to
    /// [`Excluder::is_skipped`], the same policy source as the scanner. A
    /// delete event's path can no longer be stat-ed, so only the
    /// component-level judgment applies.
    #[allow(dead_code)] // watcher 库先行：接线前仅测试与 watcher(未接线)引用
    pub fn is_excluded_path(&self, path: &Path) -> bool {
        for comp in path.components() {
            if let Component::Normal(os) = comp {
                if let Some(name) = os.to_str() {
                    if self.is_excluded_name(name) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Root-scoped component-level exclusion: excluded when the root's own
    /// basename or any relative component below it hits names/secrets.
    /// Aligned with the scanner's visible scope — walkdir's `filter_entry`
    /// sees the root itself first, and ancestor components above it are
    /// never visible to the scanner (when the root sits under an
    /// excluded-name directory like `build`/`snap`, the scanner still
    /// indexes in full, so the watcher must not silently drop a whole root's
    /// events over ancestor components). Paths outside every root are not
    /// this method's business (the caller falls back to
    /// [`Excluder::is_excluded_path`]).
    ///
    /// The extension policy is not judged here, same as
    /// [`Excluder::is_excluded_path`].
    #[allow(dead_code)] // watcher 库先行：接线前仅测试与 watcher(未接线)引用
    pub fn is_excluded_under_root(&self, path: &Path, root: &Path) -> bool {
        // The root's own basename: the scanner applies the same list to the
        // root itself (filter_entry applies to the first entry; a hit prunes
        // the whole tree).
        if let Some(name) = root.file_name().and_then(|s| s.to_str()) {
            if self.is_excluded_name(name) {
                return true;
            }
        }
        match path.strip_prefix(root) {
            Ok(rel) => rel.components().any(|comp| {
                matches!(&comp, Component::Normal(os)
                    if os.to_str().is_some_and(|n| self.is_excluded_name(n)))
            }),
            // path is not below root: the caller should have resolved root
            // ownership first; defensively fall back to the full-component
            // judgment.
            Err(_) => self.is_excluded_path(path),
        }
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
        // An ancestor component deep inside hitting means the whole path is
        // excluded
        assert!(ex.is_excluded_path(Path::new("/home/u/proj/node_modules/a/b/index.js")));
        assert!(ex.is_excluded_path(Path::new("/home/u/.ssh/config")));
        assert!(ex.is_excluded_path(Path::new("/home/u/proj/.env")));
        // The last component itself hitting (a delete event's path can no
        // longer be stat-ed, so only the component-level judgment applies)
        assert!(ex.is_excluded_path(Path::new("/home/u/proj/node_modules")));
        assert!(ex.is_excluded_path(Path::new("/home/u/.ssh")));
        // No component hits → component-level pass (the extension policy is
        // judged by is_skipped with the is_dir context)
        assert!(!ex.is_excluded_path(Path::new("/home/u/vm/disk.qcow2")));
        assert!(!ex.is_excluded_path(Path::new("/home/u/Documents/保险报价单.pdf")));
        assert!(!ex.is_excluded_path(Path::new("/home/u/Downloads/report.docx")));
        assert!(!ex.is_excluded_path(Path::new("/home/u/proj")));
    }

    /// Root-scope judgment aligns with the scanner's visible scope:
    /// ancestors above the root (`build` ∈ SKIP_NAMES) do not participate
    /// (the scanner indexes such roots as usual, the watcher must not drop
    /// whole-root events); a root whose own basename hits the list excludes
    /// the whole root (filter_entry's first-entry pruning); churn ancestors
    /// deep below the root still hit; out-of-root paths fall back to the
    /// full-component judgment.
    #[test]
    fn excluded_under_root_scopes_to_the_watched_root() {
        let ex = Excluder::default();
        // The ancestor "build" sits above the root → does not participate
        assert!(!ex.is_excluded_under_root(
            Path::new("/home/build/proj/报告.pdf"),
            Path::new("/home/build/proj"),
        ));
        // The root's own basename hits the list → whole root excluded
        // (the root itself and any depth below it)
        assert!(ex.is_excluded_under_root(
            Path::new("/home/u/node_modules"),
            Path::new("/home/u/node_modules"),
        ));
        assert!(ex.is_excluded_under_root(
            Path::new("/home/u/node_modules/pkg/deep.pdf"),
            Path::new("/home/u/node_modules"),
        ));
        // Churn ancestors deep below the root still hit
        assert!(ex.is_excluded_under_root(
            Path::new("/home/u/proj/node_modules/a/index.js"),
            Path::new("/home/u/proj"),
        ));
        // Clean paths pass (extensions not judged here, left to is_skipped)
        assert!(!ex.is_excluded_under_root(
            Path::new("/home/build/proj/src"),
            Path::new("/home/build/proj"),
        ));
        // Out-of-root paths (defensive arm) fall back to the full-component
        // judgment
        assert!(ex.is_excluded_under_root(
            Path::new("/elsewhere/.ssh/config"),
            Path::new("/home/u/proj"),
        ));
    }

    /// The extension policy shares the scanner's source (is_skipped):
    /// dot-suffixed directory names are not subject to extension constraints
    /// (the watcher once wrongly excluded `backup.old`-style directories by
    /// last-component extension, swallowing even their delete events);
    /// extensionless sensitive files are excluded as outside the whitelist
    /// (the watcher once let `.bash_history` etc. through until rescan
    /// cleared them).
    #[test]
    fn extension_policy_stays_in_is_skipped_with_kind_context() {
        let ex = Excluder::default();
        // A dot-suffixed directory name: directories are not subject to the
        // extension policy (consistent with the scanner)
        assert!(!ex.is_skipped("backup.old", true, None));
        assert!(!ex.is_skipped("会议.2024.07", true, None));
        // The same name as a file is judged by extension/whitelist
        assert!(ex.is_skipped("backup.old", false, Some("old")));
        // Blocklisted extension: file excluded
        assert!(ex.is_skipped("disk.qcow2", false, Some("qcow2")));
        // Extensionless sensitive files (outside the whitelist) excluded
        assert!(ex.is_skipped(".bash_history", false, None));
        assert!(ex.is_skipped(".git-credentials", false, None));
        assert!(ex.is_skipped(".viminfo", false, None));
    }

    #[test]
    fn whitelist_skips_dev_files() {
        let ex = Excluder::default();
        // 源码/编译产物/无扩展名 → 白名单外，排除
        assert!(ex.is_skipped("main.c", false, Some("c")));
        assert!(ex.is_skipped("App.class", false, Some("class")));
        assert!(ex.is_skipped("lib.so", false, Some("so")));
        assert!(ex.is_skipped("Makefile", false, None));
        // 目录不受白名单影响(照常下探)
        assert!(!ex.is_skipped("src", true, None));
        // 常用类型保留
        assert!(!ex.is_skipped("年报.xlsx", false, Some("xlsx")));
        assert!(!ex.is_skipped("照片.jpg", false, Some("jpg")));
        assert!(!ex.is_skipped("压缩包.zip", false, Some("zip")));
        // Watcher event level: after the component-level pass, the type
        // judgment goes through the same is_skipped as the scanner
        assert!(!ex.is_excluded_path(Path::new("/home/u/proj/main.c")));
        assert!(ex.is_skipped("main.c", false, Some("c")));
        assert!(!ex.is_excluded_path(Path::new("/home/u/proj/src")));
        assert!(!ex.is_excluded_path(Path::new("/home/u/Documents/报告.pdf")));
    }
}
