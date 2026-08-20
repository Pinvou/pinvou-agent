use super::prelude::*;
use crate::features::deliverables::DeliverableItem;
use crate::platform::path_policy::validate_user_path;
use std::sync::OnceLock;

fn artifact_lifecycle_lock() -> &'static parking_lot::Mutex<()> {
    static LOCK: OnceLock<parking_lot::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| parking_lot::Mutex::new(()))
}

/// 产物文件元数据。前端右栏 list 用。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ArtifactInfo {
    pub size: u64,
    pub kind: String,
    pub exists: bool,
    pub modified: i64,
}

/// 读 artifact 文件的纯文本（md/json/txt 等）。文件不存在或不是文本 → 报错。
/// 路径必须在用户家目录下（防 ../../../etc/passwd 之类逃逸）。
#[tauri::command]
pub async fn read_artifact_text(path: String) -> Result<String, String> {
    read_artifact_text_impl(&path)
}

pub(crate) fn read_artifact_text_impl(path: &str) -> Result<String, String> {
    let p = validate_user_path(path)?;
    let _lifecycle = artifact_lifecycle_lock().lock();
    recover_interrupted_artifact_write(&p)
        .map_err(|e| format!("recover_artifact_text({}): {e}", p.display()))?;
    std::fs::read_to_string(&p).map_err(|e| format!("read_artifact_text({}): {e}", p.display()))
}

pub(super) const MAX_EDITABLE_MARKDOWN_BYTES: usize = 10 * 1024 * 1024;

/// 写回 Markdown artifact。只允许覆盖已存在的 .md/.markdown 文件。
#[tauri::command]
pub async fn write_artifact_text(path: String, content: String) -> Result<(), String> {
    write_artifact_text_impl(&path, &content)
}

pub(crate) fn write_artifact_text_impl(path: &str, content: &str) -> Result<(), String> {
    let p = validate_user_path(path)?;
    let _lifecycle = artifact_lifecycle_lock().lock();
    recover_interrupted_artifact_write(&p)
        .map_err(|e| format!("recover_artifact_text({}): {e}", p.display()))?;
    if !p.is_file() {
        return Err(format!("not a file: {}", p.display()));
    }
    ensure_editable_artifact_path(&p)?;

    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext != "md" && ext != "markdown" {
        return Err("only markdown artifacts can be edited".into());
    }

    if content.len() > MAX_EDITABLE_MARKDOWN_BYTES {
        return Err("markdown artifact is too large to save".into());
    }

    atomic_write_utf8_unlocked(&p, content)
        .map_err(|e| format!("write_artifact_text({}): {e}", p.display()))
}

fn ensure_editable_artifact_path(path: &std::path::Path) -> Result<(), String> {
    let canonical = std::fs::canonicalize(path)
        .map_err(|e| format!("resolve artifact path({}): {e}", path.display()))?;
    let sessions_root = crate::platform::paths::sessions_root();
    let sessions_root = std::fs::canonicalize(&sessions_root).map_err(|e| {
        format!(
            "markdown artifact is outside session storage: cannot resolve sessions root({}): {e}",
            sessions_root.display()
        )
    })?;
    let rel = canonical
        .strip_prefix(&sessions_root)
        .map_err(|_| "markdown artifact is outside session storage".to_string())?;
    let mut components = rel.components();
    let session = components
        .next()
        .and_then(|c| match c {
            std::path::Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .ok_or_else(|| "markdown artifact is outside a session".to_string())?;
    if session.is_empty() || session.starts_with('_') {
        return Err("markdown artifact is outside an editable session".to_string());
    }
    let area = components
        .next()
        .and_then(|c| match c {
            std::path::Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .ok_or_else(|| "markdown artifact is outside session artifacts".to_string())?;
    if area != "artifacts" && area != "workspace" {
        return Err("markdown artifact is outside session artifacts".to_string());
    }
    Ok(())
}
pub(super) fn atomic_write_utf8(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    let _lifecycle = artifact_lifecycle_lock().lock();
    atomic_write_utf8_unlocked(path, content)
}

fn atomic_write_utf8_unlocked(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    atomic_write_utf8_unlocked_with(
        path,
        content,
        crate::platform::filesystem::replace_file_atomically,
    )
}

pub(super) fn atomic_write_utf8_unlocked_with<F>(
    path: &std::path::Path,
    content: &str,
    replace: F,
) -> std::io::Result<()>
where
    F: FnOnce(
        &std::path::Path,
        &std::path::Path,
        &std::path::Path,
    ) -> crate::platform::filesystem::ReplaceResult,
{
    use std::io::Write;

    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("artifact.md");
    let token = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let tmp = parent.join(format!(".{file_name}.tmp-{token}"));
    let backup = parent.join(format!(".{file_name}.bak-{token}"));

    let stage_result = (|| -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
        drop(f);
        Ok(())
    })();
    if let Err(error) = stage_result {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }

    match replace(&tmp, path, &backup) {
        Ok(crate::platform::filesystem::ReplaceState::Committed) => {
            let _ = std::fs::remove_file(&backup);
            if let Ok(dir) = std::fs::File::open(parent) {
                let _ = dir.sync_all();
            }
            Ok(())
        }
        Ok(state) => Err(std::io::Error::other(format!(
            "unexpected successful replacement state: {state:?}"
        ))),
        Err(error) => {
            if error.state() == crate::platform::filesystem::ReplaceState::RolledBack {
                let _ = std::fs::remove_file(&tmp);
                let _ = std::fs::remove_file(&backup);
            } else if error.state() == crate::platform::filesystem::ReplaceState::RecoveryRequired
                && path.exists()
            {
                // A target that still exists (e.g. a directory occupying its
                // path) is a permanent failure: recovery can never promote a
                // candidate over it, so the staged tmp/backup are garbage. A
                // truly missing target keeps its candidates for
                // recover_interrupted_artifact_write.
                let _ = std::fs::remove_file(&tmp);
                let _ = std::fs::remove_file(&backup);
            }
            Err(error.into_io_error())
        }
    }
}

fn recover_interrupted_artifact_write(path: &std::path::Path) -> std::io::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
        return Ok(());
    };
    let tmp_prefix = format!(".{file_name}.tmp-");
    let bak_prefix = format!(".{file_name}.bak-");
    let mut candidates = std::collections::BTreeMap::<
        String,
        (Option<std::path::PathBuf>, Option<std::path::PathBuf>),
    >::new();
    if let Ok(entries) = std::fs::read_dir(parent) {
        for entry in entries.flatten() {
            let candidate = entry.path();
            if !std::fs::symlink_metadata(&candidate)
                .is_ok_and(|metadata| metadata.file_type().is_file())
            {
                continue;
            }
            let Some(name) = candidate
                .file_name()
                .and_then(|value| value.to_str())
                .map(str::to_owned)
            else {
                continue;
            };
            if let Some(token) = name.strip_prefix(&tmp_prefix) {
                candidates.entry(token.to_string()).or_default().0 = Some(candidate);
            } else if let Some(token) = name.strip_prefix(&bak_prefix) {
                candidates.entry(token.to_string()).or_default().1 = Some(candidate);
            }
        }
    }

    if path.is_file() {
        cleanup_artifact_recovery_candidates(candidates.values());
        return Ok(());
    }

    let mut ordered = candidates
        .iter()
        .map(|(token, (tmp, backup))| {
            (
                artifact_recovery_sort_key(token, tmp.as_deref(), backup.as_deref()),
                token.clone(),
                tmp.clone(),
                backup.clone(),
            )
        })
        .collect::<Vec<_>>();
    ordered.sort_by_key(|(key, _, _, _)| *key);
    for (_, token, tmp, backup) in ordered.into_iter().rev() {
        let replacement = tmp.unwrap_or_else(|| parent.join(format!("{tmp_prefix}{token}")));
        let backup = backup.unwrap_or_else(|| parent.join(format!("{bak_prefix}{token}")));
        match crate::platform::filesystem::recover_interrupted_replace(&replacement, path, &backup)
        {
            Ok(crate::platform::filesystem::ReplaceState::Committed) => {
                cleanup_artifact_recovery_candidates(candidates.values());
                return Ok(());
            }
            Ok(_) => unreachable!("recovery success is always committed"),
            Err(error)
                if error.state() == crate::platform::filesystem::ReplaceState::RolledBack
                    && path.is_file() =>
            {
                cleanup_artifact_recovery_candidates(candidates.values());
                return Ok(());
            }
            Err(error)
                if error.state() == crate::platform::filesystem::ReplaceState::RecoveryRequired =>
            {
                return Err(error.into_io_error());
            }
            Err(error) => return Err(error.into_io_error()),
        }
    }
    Ok(())
}

fn artifact_recovery_sort_key(
    token: &str,
    tmp: Option<&std::path::Path>,
    backup: Option<&std::path::Path>,
) -> (bool, u128) {
    let timestamp = token
        .rsplit_once('-')
        .and_then(|(_, nanos)| nanos.parse::<u128>().ok())
        .or_else(|| {
            [backup, tmp]
                .into_iter()
                .flatten()
                .filter_map(|path| {
                    std::fs::metadata(path)
                        .and_then(|value| value.modified())
                        .ok()
                })
                .filter_map(|modified| {
                    modified
                        .duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .map(|duration| duration.as_nanos())
                })
                .max()
        })
        .unwrap_or_default();
    (backup.is_some(), timestamp)
}

fn cleanup_artifact_recovery_candidates<'a>(
    candidates: impl Iterator<Item = &'a (Option<std::path::PathBuf>, Option<std::path::PathBuf>)>,
) {
    for (tmp, backup) in candidates {
        if let Some(tmp) = tmp {
            let _ = std::fs::remove_file(tmp);
        }
        if let Some(backup) = backup {
            let _ = std::fs::remove_file(backup);
        }
    }
}

/// 「产出物」跨会话索引:遍历 `~/.pinvou3/sessions/*.json`,把每个会话跟踪的
/// artifacts 汇成一张扁平表(供「产出物」一级入口用)。只走磁盘真相:
/// 文件已被删则跳过;mtime/size 现取 fs。
#[tauri::command]
pub async fn list_deliverable_index() -> Result<Vec<DeliverableItem>, String> {
    Ok(crate::features::deliverables::list_deliverable_index_impl())
}

/// 读 artifact 元数据：大小 / 类型 / 是否存在。
#[tauri::command]
pub async fn artifact_info(path: String) -> Result<ArtifactInfo, String> {
    artifact_info_impl(&path)
}

/// [2026-06-07] 读图片 → base64 data url,给 FilePreviewModal 内联预览 png/jpg
/// (csp=null 不拦 data:,比 asset 协议 scope 省事)。validate_user_path 防穿越。
#[tauri::command]
pub async fn read_artifact_image_b64(path: String) -> Result<String, String> {
    let p = validate_user_path(&path).map_err(|_| "路径不允许".to_string())?;
    if !p.is_file() {
        return Err(format!("图片不存在: {path}"));
    }
    let bytes = std::fs::read(&p).map_err(|e| format!("读取失败: {e}"))?;
    if bytes.len() > 25_000_000 {
        return Err("图片过大(>25MB),请用外部打开".into());
    }
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_ascii_lowercase();
    let mime = match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        _ => "image/png",
    };
    let b64 = crate::features::files::file_ingest::base64_encode(&bytes);
    Ok(format!("data:{mime};base64,{b64}"))
}

/// [2026-06-22] pptx 封面缩略图：打开 .pptx(zip)读 `docProps/thumbnail.jpeg`
/// → base64 data url，给产物卡顶部 16:9 封面用。无缩略图 / 非 zip / 损坏 → Ok(None)
/// （前端据此回退紧凑态，不报错）。本地数据、无外链，内网离线安全。
/// validate_user_path 防路径穿越；跨平台（zip 纯 Rust，Windows/Linux 一致）。
#[tauri::command]
pub async fn read_artifact_thumbnail(path: String) -> Result<Option<String>, String> {
    use std::io::Read;
    let p = validate_user_path(&path).map_err(|_| "路径不允许".to_string())?;
    if !p.is_file() {
        return Ok(None);
    }
    let file = std::fs::File::open(&p).map_err(|e| format!("打开失败: {e}"))?;
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(z) => z,
        Err(_) => return Ok(None), // 非 zip / 损坏：前端走紧凑态
    };
    // OOXML 缩略图固定路径；兜底几种扩展名（Office 默认写 .jpeg）。
    for name in [
        "docProps/thumbnail.jpeg",
        "docProps/thumbnail.jpg",
        "docProps/thumbnail.png",
    ] {
        let mut entry = match archive.by_name(name) {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.size() > 25_000_000 {
            return Ok(None);
        }
        let mut buf = Vec::new();
        if entry.read_to_end(&mut buf).is_err() || buf.is_empty() {
            continue;
        }
        let mime = if name.ends_with(".png") {
            "image/png"
        } else {
            "image/jpeg"
        };
        let b64 = crate::features::files::file_ingest::base64_encode(&buf);
        return Ok(Some(format!("data:{mime};base64,{b64}")));
    }
    Ok(None)
}

pub(crate) fn artifact_info_impl(path: &str) -> Result<ArtifactInfo, String> {
    let p = match validate_user_path(path) {
        Ok(p) => p,
        Err(_) => {
            return Ok(ArtifactInfo {
                size: 0,
                kind: "denied".into(),
                exists: false,
                modified: 0,
            })
        }
    };
    let meta = match std::fs::metadata(&p) {
        Ok(m) => m,
        Err(_) => {
            return Ok(ArtifactInfo {
                size: 0,
                kind: "missing".into(),
                exists: false,
                modified: 0,
            })
        }
    };
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    let kind = match ext.as_str() {
        "md" | "markdown" => "md",
        "html" | "htm" => "html",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" => "image",
        "pdf" => "pdf",
        // 让前端能识别 office 格式 → 调 ingest_file 转 md 内嵌预览
        "docx" | "pptx" | "odt" => "docx",
        "xlsx" | "ods" => "xlsx",
        "doc" | "ppt" | "xls" | "rtf" => "legacy_office",
        "txt" | "log" | "csv" | "json" | "yaml" | "yml" | "toml" | "xml" | "rs" | "py" | "js"
        | "ts" | "go" | "c" | "cpp" | "h" | "hpp" | "sh" | "bash" | "zsh" | "fish" | "bat"
        | "cmd" | "ps1" | "pl" | "pm" | "lua" | "swift" | "kt" | "kts" | "scala" | "groovy"
        | "dart" | "r" | "m" | "jl" | "erl" | "hrl" | "css" | "scss" | "sass" | "less" | "vue"
        | "svelte" | "mdx" | "sql" | "ini" | "conf" | "cfg" | "env" | "properties" | "reg"
        | "diff" | "patch" | "lock" | "proto" | "graphql" | "gql" | "prisma" => "text",
        _ => "binary",
    };
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Ok(ArtifactInfo {
        size: meta.len(),
        kind: kind.into(),
        exists: true,
        modified,
    })
}

/// PDF 预览逐页转图的页数上限：太多页 data URI 会撑爆前端内存。
const VISUAL_PDF_MAX_PAGES: u32 = 30;

/// 产物可视化预览结果。前端按 `mode` 渲染。
#[derive(Debug, Clone, Serialize)]
pub struct VisualResult {
    /// "html"(iframe srcDoc 渲染) | "images"(逐张图) | "unsupported"(走统一兜底卡)
    pub mode: String,
    /// mode=html：图片已内联的自包含 HTML
    pub html: Option<String>,
    /// mode=images：图片 data URI 列表（pdf 多页 / 单图）
    pub images: Vec<String>,
    /// 缺工具 / 转换失败 / 截断 的人话提示
    pub warning: Option<String>,
}

impl VisualResult {
    fn unsupported(warning: Option<String>) -> Self {
        VisualResult {
            mode: "unsupported".into(),
            html: None,
            images: vec![],
            warning,
        }
    }
}

/// 可视化预览结果缓存（按 路径|mtime 键）。soffice/pdftoppm 一次 1-3s，缓存后二次秒开。
fn visual_cache() -> &'static parking_lot::Mutex<std::collections::HashMap<String, VisualResult>> {
    static CACHE: std::sync::OnceLock<
        parking_lot::Mutex<std::collections::HashMap<String, VisualResult>>,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(|| parking_lot::Mutex::new(std::collections::HashMap::new()))
}

/// 把 office/pdf/图片产物转成可视化预览：office→自包含 HTML，pdf→逐页 PNG，图片→data URI。
/// 结果按 路径+mtime 缓存。md/html/text 不走这里（前端直接读文本渲染）。
/// 转换慢且阻塞 → 丢到 `spawn_blocking`，不堵 tokio reactor。
#[tauri::command]
pub async fn render_artifact_visual(path: String) -> Result<VisualResult, String> {
    let p = validate_user_path(&path)?;
    if !p.is_file() {
        return Err(format!("not a file: {}", p.display()));
    }
    let mtime = std::fs::metadata(&p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let cache_key = format!("{}|{}", p.display(), mtime);
    if let Some(hit) = visual_cache().lock().get(&cache_key).cloned() {
        return Ok(hit);
    }

    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    let p2 = p.clone();
    let result = tokio::task::spawn_blocking(move || -> VisualResult {
        use crate::features::files::file_ingest as fi;
        match ext.as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" => {
                match fi::image_file_to_data_uri(&p2) {
                    Ok(uri) => VisualResult {
                        mode: "images".into(),
                        html: None,
                        images: vec![uri],
                        warning: None,
                    },
                    Err(e) => VisualResult::unsupported(Some(e)),
                }
            }
            // PDF / 演示稿 → 逐页 PNG。演示稿先转 PDF 再逐页(每页=一张幻灯片)。
            "pdf" | "pptx" | "ppt" | "odp" => {
                let conv = if ext == "pdf" {
                    fi::pdf_to_png_data_uris(&p2, VISUAL_PDF_MAX_PAGES)
                } else {
                    fi::office_to_png_data_uris(&p2, VISUAL_PDF_MAX_PAGES)
                };
                match conv {
                    Ok((imgs, truncated)) => VisualResult {
                        mode: "images".into(),
                        html: None,
                        images: imgs,
                        warning: truncated
                            .then(|| format!("页数较多，仅渲染前 {VISUAL_PDF_MAX_PAGES} 页")),
                    },
                    Err(e) => VisualResult::unsupported(Some(e)),
                }
            }
            // 文字文档 / 电子表格 → 自包含 HTML(版式 + 内联图片)。
            "docx" | "odt" | "rtf" | "doc" | "xlsx" | "ods" | "xls" => {
                match fi::libreoffice_to_inline_html(&p2) {
                    Ok(html) => VisualResult {
                        mode: "html".into(),
                        html: Some(html),
                        images: vec![],
                        warning: None,
                    },
                    Err(e) => VisualResult::unsupported(Some(e)),
                }
            }
            _ => VisualResult::unsupported(None),
        }
    })
    .await
    .map_err(|e| format!("render_artifact_visual join: {e}"))?;

    // unsupported 不缓存：可能是工具暂缺，装上后下次重试。
    if result.mode != "unsupported" {
        visual_cache().lock().insert(cache_key, result.clone());
    }
    Ok(result)
}

/// 系统没有演示文稿默认打开方式时，使用 LibreOffice 作为显式兜底。
fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn open_with_libreoffice(path: &std::path::Path) -> Result<(), String> {
    let program = crate::platform::os::libreoffice_tool_path();
    let program_text = program.to_string_lossy().to_string();
    if !crate::platform::os::command_exists(&program_text) {
        return Err(crate::platform::os::libreoffice_missing_message().into());
    }

    std::process::Command::new(&program)
        .arg(crate::platform::os::external_application_path(path))
        .spawn()
        .map_err(|e| format!("LibreOffice 打开失败: {e}"))?;
    Ok(())
}

/// 外部链接白名单：前端 webview 万一被 XSS 时的最后一道防线。
/// **扩这个列表必须同步加测试**（见 `external_allowlist_*` 单测）。
const EXTERNAL_URL_ALLOWLIST: &[&str] = &[
    "https://metaso.cn/",
    "https://open.bochaai.com/",
    "https://console.bce.baidu.com/",
    "https://app.tavily.com/",
    // 高德开放平台:天气 MCP Web 服务 Key 创建入口
    "https://console.amap.com/",
    "https://www.iwencai.com/",
    "https://agent.qcc.com/",
    // 腾讯 ima OpenAPI 凭据页
    "https://ima.qq.com/",
    // 智慧芽开放平台:智慧芽 MCP API Key 获取说明
    "https://open.zhihuiya.com/",
    // MegaCube 官网(侧边栏 footer 入口跳转)
    "https://www.h3c.com/",
    // 飞书/Lark OAuth(device flow 授权页 + 账号页);连接飞书走这里开浏览器
    "https://open.feishu.cn/",
    "https://accounts.feishu.cn/",
    "https://www.feishu.cn/",
    "https://open.larksuite.com/",
    "https://accounts.larksuite.com/",
    // 腾讯会议 OAuth 授权页；连接腾讯会议时可从流程卡打开浏览器
    "https://meeting.tencent.com/",
    // Obsidian 官网:知识库连接器探测到未安装时,引导用户下载
    "https://obsidian.md/",
    // Canva 可画 MCP 返回的设计编辑链接/预览图资源
    "https://www.canva.cn/",
    "https://export-download.canva.cn/",
    // 腾讯文档个人 Token 授权页(工具商店「腾讯文档 MCP」配置入口)
    "https://docs.qq.com/scenario/open-claw.html",
];

fn url_is_loopback_http(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return false;
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// URL 是否命中已审计外部入口或本机 loopback HTTP(S)(纯函数,便于单测)。
pub(super) fn url_in_external_allowlist(url: &str) -> bool {
    url_is_loopback_http(url) || EXTERNAL_URL_ALLOWLIST.iter().any(|p| url.starts_with(p))
}

/// 用户在 ACP 消息或产物预览中明确点击的外链。与工具可调用的固定白名单入口分开：
/// 这里只允许无 URL 凭据的 HTTP(S)，由系统浏览器承担站点隔离。
pub(super) fn user_external_url(url: &str) -> Option<reqwest::Url> {
    let raw = url.trim();
    let scheme_end = raw.find("://")?;
    let scheme = &raw[..scheme_end];
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let authority = &raw[scheme_end + 3..];
    if authority
        .chars()
        .next()
        .is_none_or(|first| first == '/' || first == '\\' || first.is_whitespace())
    {
        return None;
    }

    let parsed = reqwest::Url::parse(raw).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    Some(parsed)
}

/// 用系统默认浏览器打开已审计外部 https URL 或严格本机 loopback HTTP(S) URL。
/// 外部域名白名单写死；loopback 用于 Agent 生成页面的本地预览。
#[tauri::command]
pub async fn open_external_url(url: String) -> Result<(), String> {
    if !url_in_external_allowlist(&url) {
        return Err(format!("URL not in allowlist: {url}"));
    }
    crate::platform::os::open_target(&url, "外部链接")
}

/// 打开用户亲自点击的 ACP / 产物 HTTP(S) 外链。
/// URL 先经过严格校验，再直接交给系统默认浏览器；不得阻塞 Tauri/GTK 主事件循环。
#[tauri::command]
pub fn open_user_external_url(url: String) -> Result<(), String> {
    let parsed = user_external_url(&url).ok_or_else(|| "invalid_user_external_url".to_string())?;
    crate::platform::os::open_target(parsed.as_str(), "用户点击的外部链接")
}

/// 本机 Obsidian 状态(供工具市场"连接"前分支)。
/// state: `not_installed` | `no_vault` | `vault_missing` | `ok`
#[derive(serde::Serialize)]
pub struct ObsidianStatus {
    pub state: String,
    pub vault_path: Option<String>,
}

/// 从 obsidian.json 文本里挑出当前库路径:优先 `open:true`,否则 `ts` 最大。
/// 与 `mcp-servers/obsidian/server.py` 的 `_autodiscover_vault` 同规则,需保持一致。
pub(super) fn pick_vault_path(text: &str) -> Option<String> {
    let text = text.trim_start_matches('\u{feff}'); // 剥 BOM
    let json: serde_json::Value = serde_json::from_str(text).ok()?;
    let vaults = json.get("vaults")?.as_object()?;
    if vaults.is_empty() {
        return None;
    }
    let pick = vaults
        .values()
        .find(|v| v.get("open").and_then(|o| o.as_bool()).unwrap_or(false))
        .or_else(|| {
            vaults
                .values()
                .max_by_key(|v| v.get("ts").and_then(|t| t.as_i64()).unwrap_or(0))
        })?;
    pick.get("path")?.as_str().map(|s| s.to_string())
}

/// 探测本机 Obsidian 状态,供工具市场"连接 Obsidian"前分支:
/// 没装就引导下载,没库就引导建库,而不是默默装上一个用不了的连接器。
#[tauri::command]
pub fn detect_obsidian() -> ObsidianStatus {
    let not_installed = || ObsidianStatus {
        state: "not_installed".into(),
        vault_path: None,
    };
    let cfg = match crate::platform::os::obsidian_config_path() {
        Some(p) if p.is_file() => p,
        _ => return not_installed(),
    };
    let text = match std::fs::read_to_string(&cfg) {
        Ok(t) => t,
        Err(_) => return not_installed(),
    };
    match pick_vault_path(&text) {
        None => ObsidianStatus {
            state: "no_vault".into(),
            vault_path: None,
        },
        Some(p) if std::path::Path::new(&p).is_dir() => ObsidianStatus {
            state: "ok".into(),
            vault_path: Some(p),
        },
        Some(p) => ObsidianStatus {
            state: "vault_missing".into(),
            vault_path: Some(p),
        },
    }
}

/// 把成品卡里可能的**相对**路径落到产物所属 session 的 workspace。
///
/// 背景:present_artifact 没调成(模型把工具名漂成 `pinvou-present_artifact` 之类
/// → NotAvailable)时,成品卡由 write_file 兜底补出,path 直接用了 write_file 的
/// 相对参数(如 `snake-game.html`)。点 Open 把相对路径丢给 `validate_user_path`
/// → 直接拒「path must be absolute」。这里先按 workspace 解析,绝对路径原样返回
/// (present_artifact 成功解析的 / 产物面板 list_workspace_files 给的已是绝对)。
///
/// `session_id` = 卡片携带的**产物所属** session,**优先**用它而非全局 active_id:
/// 切回「已访问过(有 buffer)」的会话时,前端走 switchActiveTo 不调 load_session,
/// 后端 active_id 不更新 → 仍指向切走时去的那个 session → 相对路径被拼到错的
/// workspace(报「not a file」)。卡片自带 session 才能跨会话切换稳定解析。
/// None 时(老卡无此字段 / 绝对路径)回退 active_id,行为同旧版。
pub(super) fn resolve_artifact_path(
    raw: &str,
    session_id: Option<&str>,
    store: &SessionStore,
) -> Result<String, String> {
    if std::path::Path::new(raw).is_absolute() {
        return Ok(raw.to_string());
    }
    let sid = session_id
        .map(|s| s.to_string())
        .or_else(|| store.active_id());
    match sid {
        Some(sid) => store
            .ledger_root(&sid)
            .map(|workspace| resolve_artifact_path_in_workspace(raw, &workspace))
            .map_err(|error| format!("resolve ledger root for {sid}: {error:#}")),
        None => Ok(raw.to_string()),
    }
}

pub(crate) fn resolve_artifact_path_in_workspace(raw: &str, workspace: &std::path::Path) -> String {
    crate::platform::path_policy::resolve_artifact_path_in_workspace(raw, workspace)
}

/// 用系统默认应用打开文件；
/// 相对路径先按产物所属 session（前端传 `sessionId`，缺则 active）的 workspace 解析。
#[tauri::command]
pub async fn open_in_system(
    path: String,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    let resolved = resolve_artifact_path(&path, session_id.as_deref(), &store)?;
    let p = validate_user_path(&resolved)?;
    if crate::platform::os::libreoffice_open_fallback_needed(&p) {
        return open_with_libreoffice(&p);
    }
    crate::platform::os::open_target(crate::platform::os::external_application_path(&p), "产物")
}

/// 用文件管理器打开**所在目录**（不是文件本身）。
#[tauri::command]
pub async fn open_containing_folder(
    path: String,
    session_id: Option<String>,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    let resolved = resolve_artifact_path(&path, session_id.as_deref(), &store)?;
    let p = validate_user_path(&resolved)?;
    let dir = p
        .parent()
        .ok_or_else(|| format!("no parent dir for {}", p.display()))?;
    crate::platform::os::open_target(
        crate::platform::os::external_application_path(dir),
        "产物所在目录",
    )
}

/// 在文件管理器里定位 session 文件夹。对标 WorkBuddy:打开所有任务文件夹的上级目录,
/// 并尽可能选中当前任务文件夹；Linux 文件管理器不支持选中时退回打开 sessions 根目录。
#[tauri::command]
pub async fn reveal_session_folder(
    session_id: String,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    if !valid_session_id(&session_id) {
        return Err("invalid session id".into());
    }
    store
        .load(&session_id)
        .map_err(|e| format!("load_session({session_id}): {e:#}"))?;
    // 定时运行会话没有独立 runtime 目录，打开它所属任务的共享工作间。
    if store.scheduled_profile(&session_id).is_some() {
        let dir = store
            .ledger_root(&session_id)
            .map_err(|e| format!("reveal_session_folder({session_id}): {e:#}"))?;
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("create scheduled task workspace {}: {e}", dir.display()))?;
        return crate::platform::os::open_target(
            crate::platform::os::external_application_path(&dir),
            "定时任务工作区",
        );
    }
    let dir = crate::platform::paths::sessions_root().join(&session_id);
    if !dir.is_dir() {
        return Err(format!("session folder not found: {}", dir.display()));
    }
    crate::platform::os::reveal_target(&dir)
}

/// 打开某个定时任务独享的工作间。工作间由 automation id 稳定派生，任务的多次运行
/// 共享该目录；首次打开早于首次运行时按需创建，不接受前端传入任意文件系统路径。
#[tauri::command]
pub async fn open_scheduled_task_folder(automation_id: String) -> Result<(), String> {
    if !valid_session_id(&automation_id) {
        return Err("invalid automation id".into());
    }
    let dir = crate::platform::paths::scheduled_task_workspace_dir(&automation_id);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("create scheduled task workspace {}: {e}", dir.display()))?;
    crate::platform::os::open_target(
        crate::platform::os::external_application_path(&dir),
        "定时任务工作区",
    )
}

/// 在 Tauri 新窗口里加载 HTML 产物。绕过 snap 浏览器对 `~/.xxx/` 隐藏目录的沙箱限制。
/// 同一文件再次调用 → focus 已有窗口而非新建,防窗口爆炸。
#[tauri::command]
pub async fn open_artifact_window(
    path: String,
    session_id: Option<String>,
    app: tauri::AppHandle,
    store: State<'_, SessionStore>,
) -> Result<(), String> {
    use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

    let resolved = resolve_artifact_path(&path, session_id.as_deref(), &store)?;
    let p = validate_user_path(&resolved)?;
    if !p.is_file() {
        return Err(format!("not a file: {}", p.display()));
    }
    if crate::platform::os::system_default_open_supported(&p) {
        return crate::platform::os::open_target(
            crate::platform::os::external_application_path(&p),
            "产物",
        );
    }
    if crate::platform::os::libreoffice_open_fallback_needed(&p) {
        return open_with_libreoffice(&p);
    }
    // 用文件 inode 做稳定 label,防同一文件多次打开建多窗口。Tauri label 只允许 a-zA-Z0-9-_。
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    p.to_string_lossy().hash(&mut hasher);
    let label = format!("artifact-{:x}", hasher.finish());

    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.set_focus();
        return Ok(());
    }

    let url = crate::platform::os::file_url_from_path(&p)?;
    let title = p
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("产物")
        .to_string();

    WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(url))
        .title(title)
        .inner_size(1024.0, 768.0)
        .center()
        .resizable(true)
        .build()
        .map_err(|e| format!("build artifact window: {e}"))?;
    Ok(())
}
