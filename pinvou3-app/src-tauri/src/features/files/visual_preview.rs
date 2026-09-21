//! 可视化预览与 OCR：产物图片内联、office/PDF 逐页 PNG、扫描件 OCR 兜底。
//!
//! 对外暴露的 pub 面（供 `commands::artifacts` 渲染产物可视化复用）：
//! [`image_file_to_data_uri`] / [`libreoffice_to_inline_html`] /
//! [`office_to_png_data_uris`] / [`pdf_to_png_data_uris`] / [`ocr_image_for_kb`]。
//!
//! 对 facade 暴露 [`ingest_image`]（图片元数据登记）与 [`ocr_pdf`]（被
//! [`super::ingest_pdf`] 在无文字层时调用）。

use std::path::{Path, PathBuf};

use base64::Engine as _;

use super::IngestResult;
use super::ingest_deps::{
    add_ocr_tessdata_arg, ocr_lang_arg, ocr_tool_command, pdf_tool_command, system_tools,
};
// The LibreOffice primitives are only exercised directly by the cfg(test)
// conversion tests below; production paths go through run_libreoffice_convert.
#[cfg(test)]
use super::ingest_deps::{libreoffice_tool_command, libreoffice_user_installation_arg};

// ============== 产物可视化预览助手（commands::render_artifact_visual 复用）==============

/// Image extension → MIME. Used for data URI prefixes; the mapping itself is
/// shared with codex_acp::attachments; extensions outside the table fall back to application/octet-stream.
fn image_mime(ext: &str) -> &'static str {
    // image_mime_type takes the extension via Path::extension(); add a no-extension prefix so
    // `ext` lands in the extension position (ext comes from path.extension(), without separators or dot).
    let probe = Path::new("img").with_extension(ext);
    crate::platform::filesystem::image_mime_type(&probe).unwrap_or("application/octet-stream")
}

/// 单个图片文件 → `data:image/...;base64,...`。
pub fn image_file_to_data_uri(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读图失败: {e}"))?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    Ok(format!(
        "data:{};base64,{}",
        image_mime(ext),
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

/// 把 HTML 里指向本地旁置图片的 `src` 引用 base64 内联,产出自包含 HTML。
/// soffice 导出的 HTML 把图片写成 `<stem>_html_xxx.png` 同目录文件,iframe `srcDoc`
/// 加载不到这些相对路径 → 全部内联。已是 data:/http 的跳过。双引号、单引号都处理。
fn inline_html_images(html: &str, dir: &Path) -> String {
    let mut result = html.to_string();
    for quote in ['"', '\''] {
        let needle = format!("src={quote}");
        let mut rebuilt = String::with_capacity(result.len());
        let mut from = 0;
        loop {
            match result[from..].find(&needle) {
                Some(rel) => {
                    let val_start = from + rel + needle.len();
                    match result[val_start..].find(quote) {
                        Some(endrel) => {
                            let val_end = val_start + endrel;
                            let val = &result[val_start..val_end];
                            rebuilt.push_str(&result[from..val_start]); // 含 src="
                            if val.starts_with("data:") || val.starts_with("http") || val.is_empty()
                            {
                                rebuilt.push_str(val);
                            } else {
                                let fname = val.trim_start_matches("./");
                                match image_file_to_data_uri(&dir.join(fname)) {
                                    Ok(uri) => rebuilt.push_str(&uri),
                                    Err(_) => rebuilt.push_str(val),
                                }
                            }
                            from = val_end; // 闭合引号留给下一轮拼接
                        }
                        None => {
                            rebuilt.push_str(&result[from..]);
                            break;
                        }
                    }
                }
                None => {
                    rebuilt.push_str(&result[from..]);
                    break;
                }
            }
        }
        result = rebuilt;
    }
    result
}

/// office 文档 → 可视化 HTML（版式/图片还原）。soffice `--convert-to html`,旁置图片
/// inlined into a self-contained HTML returned to the frontend, which feeds it directly to iframe srcDoc. Reuses the dedicated UserInstallation profile
/// + temp-dir conventions (see [`super::ingest_deps::run_libreoffice_convert`]).
pub fn libreoffice_to_inline_html(path: &Path) -> Result<String, String> {
    if !system_tools().libreoffice {
        return Err(crate::platform::os::libreoffice_missing_message().into());
    }
    // Do not hardcode `html:HTML` (that filter is Writer-specific; applying it to Calc/Impress produces no output).
    // Pass only `html` → LibreOffice automatically picks the matching HTML export filter by document type.
    super::ingest_deps::run_libreoffice_convert(
        path,
        "html",
        "pinvou3-lo-html",
        "LibreOffice 转换失败",
        |tmpdir| {
            // Do not assume the output is named `<stem>.html` (different filters / special characters in file names can both change it) —
            // scan the temp dir for the produced .html (prefer a stem match, otherwise take the first one).
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let htmls: Vec<PathBuf> = std::fs::read_dir(tmpdir)
                .map(|rd| {
                    rd.filter_map(|e| e.ok().map(|e| e.path()))
                        .filter(|p| {
                            matches!(
                                p.extension().and_then(|e| e.to_str()),
                                Some("html") | Some("htm")
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            let pick = htmls
                .iter()
                .find(|p| p.file_stem().and_then(|s| s.to_str()) == Some(stem))
                .or_else(|| htmls.first())
                .ok_or_else(|| "LibreOffice 未产出 HTML".to_string())?;
            let html =
                std::fs::read_to_string(pick).map_err(|e| format!("读取转换 HTML 失败: {e}"))?;
            Ok(inline_html_images(&html, tmpdir))
        },
    )
}

/// 演示稿(pptx/ppt/odp)→ 先转 PDF 再逐页转 PNG。Impress 的 HTML 导出会拆成一堆
/// 文件且版式失真,转 PDF→PNG 更可靠:每页 = 一张幻灯片。复用 110 dpi。
pub fn office_to_png_data_uris(path: &Path, max_pages: u32) -> Result<(Vec<String>, bool), String> {
    let tools = system_tools();
    if !tools.libreoffice {
        return Err(crate::platform::os::libreoffice_missing_message().into());
    }
    if !tools.pdftoppm {
        return Err(crate::platform::os::pdf_render_missing_message().into());
    }
    // office → PDF → PNG pages share one temp dir (render pages in place once the PDF lands, avoiding another directory).
    super::ingest_deps::run_libreoffice_convert(
        path,
        "pdf",
        "pinvou3-office-png",
        "LibreOffice 转 PDF 失败",
        |tmpdir| {
            // Locate the produced PDF (scan the directory; do not assume a file name).
            let pdf = std::fs::read_dir(tmpdir)
                .ok()
                .and_then(|rd| {
                    rd.filter_map(|e| e.ok().map(|e| e.path()))
                        .find(|p| p.extension().and_then(|e| e.to_str()) == Some("pdf"))
                })
                .ok_or_else(|| "LibreOffice 未产出 PDF".to_string())?;

            let pages = render_pdf_pages(&pdf, tmpdir, 110, max_pages)?;
            if pages.is_empty() {
                return Err("未产出可渲染幻灯片页".into());
            }
            let truncated = pages.len() as u32 >= max_pages;
            let mut uris = Vec::with_capacity(pages.len());
            for p in &pages {
                uris.push(image_file_to_data_uri(p)?);
            }
            Ok((uris, truncated))
        },
    )
}

/// Shared pdftoppm rendering core: renders `pdf` into `dir` at `dpi` (`page-<n>.png`,
/// capped at `max_pages` pages), returning page paths sorted by file name. The exit/spawn-failure message strings
/// are the historical wording shared by all three callers.
fn render_pdf_pages(
    pdf: &Path,
    dir: &Path,
    dpi: u32,
    max_pages: u32,
) -> Result<Vec<PathBuf>, String> {
    let prefix = dir.join("page");
    // pdftoppm -png -r <dpi> -l <max> <pdf> <prefix> → page-1.png, page-2.png ...
    // pdftoppm 卡死按超时 kill-tree（120s,与 #532 的各内联转换点同预算）。
    let convert = crate::platform::process::output_with_timeout_and_kill_tree(
        pdf_tool_command("pdftoppm")
            .arg("-png")
            .arg("-r")
            .arg(dpi.to_string())
            .arg("-l")
            .arg(max_pages.to_string())
            .arg(pdf)
            .arg(&prefix),
        std::time::Duration::from_secs(120),
    );
    match convert {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            return Err(format!(
                "pdftoppm 转图失败: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ));
        }
        Err(e) => return Err(format!("pdftoppm 调用失败: {e}")),
    }
    // Collect the generated pngs, sorted by file name to guarantee page order.
    let mut pages: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.extension().and_then(|e| e.to_str()) == Some("png")
                        && p.file_stem()
                            .and_then(|s| s.to_str())
                            .map(|s| s.starts_with("page"))
                            .unwrap_or(false)
                })
                .collect()
        })
        .unwrap_or_default();
    pages.sort();
    Ok(pages)
}

/// PDF → list of per-page PNG data URIs (visual preview). Reuses [`render_pdf_pages`]'s
/// pdftoppm invocation boilerplate, but at 110 dpi (clear enough for preview without overly large data URIs). Returns (data_uris, whether truncated at the cap).
pub fn pdf_to_png_data_uris(path: &Path, max_pages: u32) -> Result<(Vec<String>, bool), String> {
    if !system_tools().pdftoppm {
        return Err(crate::platform::os::pdf_render_missing_message().into());
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmpdir = std::env::temp_dir().join(format!("pinvou3-pdfpreview-{ts}"));
    std::fs::create_dir_all(&tmpdir).map_err(|e| format!("创建临时目录失败: {e}"))?;

    let result = (|| -> Result<(Vec<String>, bool), String> {
        let pages = render_pdf_pages(path, &tmpdir, 110, max_pages)?;
        if pages.is_empty() {
            return Err("PDF 未产出可渲染页".into());
        }
        let truncated = pages.len() as u32 >= max_pages;
        let mut uris = Vec::with_capacity(pages.len());
        for p in &pages {
            uris.push(image_file_to_data_uri(p)?);
        }
        Ok((uris, truncated))
    })();
    let _ = std::fs::remove_dir_all(&tmpdir);
    result
}

// ============== 图片摄入 + OCR 兜底 ==============

/// 图片：Qwen3.6 有视觉能力(2026-05-28 实证),不再跑 OCR 降级。这里只登记元
/// 数据,真正"看图"由 LLM 在对话里调 `image_analyze` 完成——commands.rs 发消息时
/// 会把图拷进 session workspace 的 `attachments/` 并给出相对路径引导。
/// markdown 留空(不预解析像素),token_estimate=0(视觉 token 量取决于分辨率,
/// 不在此处解码估算,UI 计数会略低,属已知局限)。
pub(super) fn ingest_image(
    _path: &Path,
    basename: String,
    path_str: String,
    byte_size: u64,
) -> IngestResult {
    IngestResult::placeholder("image", &basename, Path::new(&path_str), byte_size)
}

/// 对单张图片跑 tesseract，识别文字到 stdout。`tesseract <img> - -l <langs>`。
/// 单页识别是秒级操作，60s 兜底 + kill-tree：挂死的 tesseract 不得拖死整条
/// OCR 链（页数封顶见 [`ocr_pdf`]）。
fn ocr_image(path: &Path) -> Result<String, String> {
    let lang = ocr_lang_arg();
    let mut command = ocr_tool_command();
    command.arg(path).arg("-").arg("-l").arg(&lang);
    add_ocr_tessdata_arg(&mut command);
    let out = crate::platform::process::output_with_timeout_and_kill_tree(
        command,
        std::time::Duration::from_secs(60),
    )
    .map_err(|e| format!("tesseract 调用失败: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    } else {
        Err(format!(
            "tesseract 退出码 {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// 扫描件 PDF OCR 兜底：pdftoppm 逐页转 PNG（150 dpi），每页跑 tesseract，拼接。
/// 页数封顶 PDF_OCR_MAX_PAGES，超出截断并在末尾标注，避免几十页扫描件把上下文撑爆。
/// 知识库专用：对图片做 OCR 取文字。**只在 KB 入库调**——对话附件图仍走视觉(image_analyze)，
/// `ingest_image` 不在这里 OCR（保留 2026-05-28「图片不预解析、交给视觉」的对话侧设计）。
/// 没装 tesseract / OCR 失败 / 识别为空 → None（调用方落 skipped）。
pub fn ocr_image_for_kb(path: &Path) -> Option<String> {
    if !system_tools().tesseract {
        return None;
    }
    match ocr_image(path) {
        Ok(t) if !t.trim().is_empty() => Some(t),
        _ => None,
    }
}

/// 扫描件 PDF OCR 兜底（被 [`super::ingest_pdf`] 在 pdftotext 空白时调用）。
/// pdftoppm 逐页转 PNG（150 dpi），每页跑 tesseract，拼接。
pub(super) fn ocr_pdf(
    path: &Path,
    basename: String,
    path_str: String,
    byte_size: u64,
) -> IngestResult {
    const PDF_OCR_MAX_PAGES: u32 = 30;
    let tools = system_tools();
    // 构造器从 &Path 还原 path 字符串；path_str 来自上游 path.to_string_lossy()，
    // 用 Path::new(&path_str) 复用同一字符串视图，保证 path 字段逐字节一致。
    let result_path = Path::new(&path_str);
    if !tools.tesseract || !tools.pdftoppm {
        return IngestResult::warning(
            "pdf",
            &basename,
            result_path,
            byte_size,
            crate::platform::os::pdf_ocr_missing_message(),
        );
    }

    // 临时目录：每次唯一，避免并发冲突。
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmpdir = std::env::temp_dir().join(format!("pinvou3-pdfocr-{ts}"));
    if let Err(e) = std::fs::create_dir_all(&tmpdir) {
        return IngestResult::warning(
            "pdf",
            &basename,
            result_path,
            byte_size,
            format!("创建临时目录失败: {e}"),
        );
    }

    let rendered = render_pdf_pages(path, &tmpdir, 150, PDF_OCR_MAX_PAGES);
    let result = match rendered {
        Ok(pages) => {
            if pages.is_empty() {
                IngestResult::warning(
                    "pdf",
                    &basename,
                    result_path,
                    byte_size,
                    "PDF 无文字层，且 pdftoppm 未产出可识别页",
                )
            } else {
                let mut parts = Vec::new();
                // OCR 失败（含 tesseract 超时被 kill）必须与「空白页」区分：
                // 部分页静默丢失会让用户/模型误以为内容完整。
                let mut failed_pages = 0usize;
                for (idx, page) in pages.iter().enumerate() {
                    match ocr_image(page) {
                        Ok(text) if !text.trim().is_empty() => {
                            parts.push(format!("## 第 {} 页\n\n{}", idx + 1, text.trim()));
                        }
                        Ok(_) => {}
                        Err(_) => failed_pages += 1,
                    }
                }
                let mut content = parts.join("\n\n");
                if pages.len() as u32 >= PDF_OCR_MAX_PAGES {
                    content.push_str(&format!(
                        "\n\n> ⚠️ 扫描件页数较多，OCR 仅处理前 {PDF_OCR_MAX_PAGES} 页"
                    ));
                }
                // 仅「部分成功 + 部分失败」时在正文里注记；全部失败时 parts
                // 为空，走下方「未识别到文字」警告，失败注记不充当正文。
                if failed_pages > 0 && !parts.is_empty() {
                    content.push_str(&format!(
                        "\n\n> ⚠️ 有 {failed_pages} 页 OCR 处理失败（可能超时），内容可能不完整"
                    ));
                }
                if content.trim().is_empty() {
                    IngestResult::warning(
                        "pdf",
                        &basename,
                        result_path,
                        byte_size,
                        "扫描件 OCR 未识别到文字",
                    )
                } else {
                    // 同时带 markdown 与 warning（OCR 误差提示），不符合任何构造器，
                    // 保留字面量 —— 这是「正文 + 告警」的特例。
                    let tokens = super::estimate_tokens(&content);
                    IngestResult {
                        kind: "pdf".into(),
                        basename,
                        path: path_str.clone(),
                        markdown: Some(content),
                        token_estimate: tokens,
                        byte_size,
                        warning: Some("扫描件 PDF，内容由 OCR 提取，可能有识别误差".into()),
                    }
                }
            }
        }
        Err(message) => IngestResult::warning("pdf", &basename, result_path, byte_size, message),
    };

    let _ = std::fs::remove_dir_all(&tmpdir);
    result
}

#[cfg(test)]
mod visual_preview_smoke {
    use super::*;
    use std::process::Command;

    // 真跑 soffice/pandoc/pdftoppm，验证可视化预览两条路径产出非空 + 图片内联。
    // 依赖系统工具，CI 无则 ignore。
    #[test]
    #[ignore = "需要 libreoffice + pandoc + poppler"]
    fn office_to_inline_html_and_pdf_to_pngs() {
        let dir = std::env::temp_dir().join(format!("pinvou3-visual-smoke-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // 1) md -> docx (pandoc) -> inline html
        let md = dir.join("doc.md");
        std::fs::write(&md, "# 标题\n\n正文一段。\n\n- 列表项\n").unwrap();
        let docx = dir.join("doc.docx");
        let ok = Command::new("pandoc")
            .arg(&md)
            .arg("-o")
            .arg(&docx)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(ok && docx.exists(), "pandoc 造 docx 失败");
        let html = libreoffice_to_inline_html(&docx).expect("office->html 应成功");
        assert!(html.contains("标题"), "HTML 应含正文文字");
        assert!(
            !html.contains("src=\"doc_html"),
            "旁置图片应已内联(不应残留相对 src)"
        );

        // 2) md -> pdf (soffice via docx) -> png 页
        let pdf = dir.join("doc.pdf");
        let _ = libreoffice_tool_command()
            .arg(libreoffice_user_installation_arg(&dir.join("p")).unwrap())
            .args(["--headless", "--convert-to", "pdf", "--outdir"])
            .arg(&dir)
            .arg(&docx)
            .status();
        if pdf.exists() {
            let (imgs, _trunc) = pdf_to_png_data_uris(&pdf, 30).expect("pdf->png 应成功");
            assert!(!imgs.is_empty(), "应产出至少一页");
            assert!(
                imgs[0].starts_with("data:image/png;base64,"),
                "应为 png data URI"
            );
        }

        // 3) md -> pptx (pandoc) -> office_to_png(演示稿走 PDF→PNG)
        let pptx = dir.join("deck.pptx");
        if Command::new("pandoc")
            .arg(&md)
            .arg("-o")
            .arg(&pptx)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            let (imgs, _t) = office_to_png_data_uris(&pptx, 30).expect("pptx->png 应成功");
            assert!(
                !imgs.is_empty() && imgs[0].starts_with("data:image/png;base64,"),
                "pptx 应产出页图"
            );
        }

        // 4) csv -> xlsx (soffice) -> inline html(电子表格走 HTML 表格)
        let csv = dir.join("data.csv");
        std::fs::write(&csv, "甲,乙\n1,2\n3,4\n").unwrap();
        let _ = libreoffice_tool_command()
            .arg(libreoffice_user_installation_arg(&dir.join("p2")).unwrap())
            .args(["--headless", "--convert-to", "xlsx", "--outdir"])
            .arg(&dir)
            .arg(&csv)
            .status();
        let xlsx = dir.join("data.xlsx");
        if xlsx.exists() {
            let html = libreoffice_to_inline_html(&xlsx).expect("xlsx->html 应成功");
            assert!(
                html.contains('甲') || html.to_lowercase().contains("table"),
                "xlsx HTML 应含表格内容"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
