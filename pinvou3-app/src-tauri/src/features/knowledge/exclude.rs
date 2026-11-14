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

    /// 组件级排除：路径的**每一级** basename（含末级）命中 names/secrets 即排除。
    /// watcher 用——递归监听不会自动剪枝，事件里仍会冒出 node_modules/.ssh 等深处的路径，
    /// 扫描器走 walkdir `filter_entry` 逐层剪枝，无此需要。
    ///
    /// 扩展名策略不在此判：它需要 `is_dir` 上下文（目录不受黑/白名单约束，否则
    /// `backup.old` 这类带点目录会被误排），由调用方 stat 后交 [`Excluder::is_skipped`]
    /// 与扫描器同一策略源判定；删除事件的路径已不可 stat，只做组件级判定。
    #[allow(dead_code)] // watcher 库先行：接线前仅测试与 watcher(未接线)引用
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
        // 深处的祖先组件命中即整条路径排除
        assert!(ex.is_excluded_path(Path::new("/home/u/proj/node_modules/a/b/index.js")));
        assert!(ex.is_excluded_path(Path::new("/home/u/.ssh/config")));
        assert!(ex.is_excluded_path(Path::new("/home/u/proj/.env")));
        // 末级组件本身命中（删除事件的路径已不可 stat，只能靠组件级判定）
        assert!(ex.is_excluded_path(Path::new("/home/u/proj/node_modules")));
        assert!(ex.is_excluded_path(Path::new("/home/u/.ssh")));
        // 无组件命中 → 组件级放行（扩展名策略由 is_skipped 按 is_dir 上下文判定）
        assert!(!ex.is_excluded_path(Path::new("/home/u/vm/disk.qcow2")));
        assert!(!ex.is_excluded_path(Path::new("/home/u/Documents/保险报价单.pdf")));
        assert!(!ex.is_excluded_path(Path::new("/home/u/Downloads/report.docx")));
        assert!(!ex.is_excluded_path(Path::new("/home/u/proj")));
    }

    /// 扩展名策略与扫描器同源（is_skipped）：带点目录名不受扩展名约束（watcher
    /// 曾按末级扩展名误排 `backup.old` 这类目录、连其删除事件一起吞掉）；无扩展名
    /// 敏感文件按白名单外排除（watcher 曾放行 `.bash_history` 等直至重扫才清掉）。
    #[test]
    fn extension_policy_stays_in_is_skipped_with_kind_context() {
        let ex = Excluder::default();
        // 带点目录名：目录不受扩展名策略约束（与扫描器一致）
        assert!(!ex.is_skipped("backup.old", true, None));
        assert!(!ex.is_skipped("会议.2024.07", true, None));
        // 同名作为文件则按扩展名/白名单判
        assert!(ex.is_skipped("backup.old", false, Some("old")));
        // 黑名单扩展名：文件排除
        assert!(ex.is_skipped("disk.qcow2", false, Some("qcow2")));
        // 无扩展名敏感文件（白名单外）排除
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
        // watcher 事件级：组件级放行后，类型判定与扫描器走同一个 is_skipped
        assert!(!ex.is_excluded_path(Path::new("/home/u/proj/main.c")));
        assert!(ex.is_skipped("main.c", false, Some("c")));
        assert!(!ex.is_excluded_path(Path::new("/home/u/proj/src")));
        assert!(!ex.is_excluded_path(Path::new("/home/u/Documents/报告.pdf")));
    }
}
