//! 可视化预览与 OCR：产物图片内联、office/PDF 逐页 PNG、扫描件 OCR 兜底。
//!
//! 对外暴露的 pub 面（供 `commands::artifacts` 渲染产物可视化复用）：
//! [`base64_encode`] / [`image_file_to_data_uri`] / [`libreoffice_to_inline_html`] /
//! [`office_to_png_data_uris`] / [`pdf_to_png_data_uris`] / [`ocr_image_for_kb`]。
//!
//! 对 facade 暴露 [`ingest_image`]（图片元数据登记）与 [`ocr_pdf`]（被
//! [`super::ingest_pdf`] 在无文字层时调用）。

use std::path::{Path, PathBuf};

use super::ingest_deps::{
    add_ocr_tessdata_arg, libreoffice_tool_command, libreoffice_user_installation_arg,
    ocr_lang_arg, ocr_tool_command, pdf_tool_command, system_tools,
};
use super::IngestResult;

// ============== 产物可视化预览助手（commands::render_artifact_visual 复用）==============

/// 极简 base64 标准编码（无换行）。仅为内联图片 data URI 用，不值得引第三方 crate。
pub fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// 图片扩展名 → MIME。用于 data URI 前缀。
fn image_mime(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// 单个图片文件 → `data:image/...;base64,...`。
pub fn image_file_to_data_uri(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读图失败: {e}"))?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    Ok(format!(
        "data:{};base64,{}",
        image_mime(ext),
        base64_encode(&bytes)
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
/// 内联成自包含 HTML 返回,前端直接喂 iframe srcDoc。复用 libreoffice_convert_text 的
/// 独立 UserInstallation profile + 临时目录约定。
pub fn libreoffice_to_inline_html(path: &Path) -> Result<String, String> {
    if !system_tools().libreoffice {
        return Err(crate::platform::os::libreoffice_missing_message().into());
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmpdir = std::env::temp_dir().join(format!("pinvou3-lo-html-{ts}"));
    std::fs::create_dir_all(&tmpdir).map_err(|e| format!("创建临时目录失败: {e}"))?;

    let out = libreoffice_tool_command()
        .arg(libreoffice_user_installation_arg(&tmpdir.join("profile"))?)
        .arg("--headless")
        .arg("--convert-to")
        // 不写死 `html:HTML`(那是 Writer 专用 filter,套到 Calc/Impress 会无产出)。
        // 只给 `html` → LibreOffice 按文档类型自动选对应 HTML 导出 filter。
        .arg("html")
        .arg("--outdir")
        .arg(&tmpdir)
        .arg(path)
        .output();

    let result = (|| -> Result<String, String> {
        match out {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                return Err(format!(
                    "LibreOffice 转换失败: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                ))
            }
            Err(e) => return Err(format!("LibreOffice 调用失败: {e}")),
        }
        // 不假设产物叫 `<stem>.html`(filter 不同 / 文件名带特殊字符都可能变)——
        // 扫临时目录里产出的 .html(优先匹配 stem,否则取首个)。
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let htmls: Vec<PathBuf> = std::fs::read_dir(&tmpdir)
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
        let html = std::fs::read_to_string(pick).map_err(|e| format!("读取转换 HTML 失败: {e}"))?;
        Ok(inline_html_images(&html, &tmpdir))
    })();
    let _ = std::fs::remove_dir_all(&tmpdir);
    result
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
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmpdir = std::env::temp_dir().join(format!("pinvou3-office-png-{ts}"));
    std::fs::create_dir_all(&tmpdir).map_err(|e| format!("创建临时目录失败: {e}"))?;

    let result = (|| -> Result<(Vec<String>, bool), String> {
        // 1) office → PDF
        let out = libreoffice_tool_command()
            .arg(libreoffice_user_installation_arg(&tmpdir.join("profile"))?)
            .arg("--headless")
            .arg("--convert-to")
            .arg("pdf")
            .arg("--outdir")
            .arg(&tmpdir)
            .arg(path)
            .output();
        match out {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                return Err(format!(
                    "LibreOffice 转 PDF 失败: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                ))
            }
            Err(e) => return Err(format!("LibreOffice 调用失败: {e}")),
        }
        // 找产出的 PDF(扫目录,别假设文件名)。
        let pdf = std::fs::read_dir(&tmpdir)
            .ok()
            .and_then(|rd| {
                rd.filter_map(|e| e.ok().map(|e| e.path()))
                    .find(|p| p.extension().and_then(|e| e.to_str()) == Some("pdf"))
            })
            .ok_or_else(|| "LibreOffice 未产出 PDF".to_string())?;

        // 2) PDF → PNG 页(直接在同一 tmpdir,避免再开目录)。
        let prefix = tmpdir.join("page");
        let conv = pdf_tool_command("pdftoppm")
            .arg("-png")
            .arg("-r")
            .arg("110")
            .arg("-l")
            .arg(max_pages.to_string())
            .arg(&pdf)
            .arg(&prefix)
            .output();
        match conv {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                return Err(format!(
                    "pdftoppm 转图失败: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                ))
            }
            Err(e) => return Err(format!("pdftoppm 调用失败: {e}")),
        }
        let mut pages: Vec<PathBuf> = std::fs::read_dir(&tmpdir)
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
        if pages.is_empty() {
            return Err("未产出可渲染幻灯片页".into());
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

/// PDF → 逐页 PNG 的 data URI 列表(可视化预览)。复用 ocr_pdf 的 pdftoppm 调用样板,
/// 但 110 dpi(预览够清又不至于 data URI 过大)。返回 (data_uris, 是否因上限截断)。
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
    let prefix = tmpdir.join("page");

    let convert = pdf_tool_command("pdftoppm")
        .arg("-png")
        .arg("-r")
        .arg("110")
        .arg("-l")
        .arg(max_pages.to_string())
        .arg(path)
        .arg(&prefix)
        .output();

    let result = (|| -> Result<(Vec<String>, bool), String> {
        match convert {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                return Err(format!(
                    "pdftoppm 转图失败: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                ))
            }
            Err(e) => return Err(format!("pdftoppm 调用失败: {e}")),
        }
        let mut pages: Vec<PathBuf> = std::fs::read_dir(&tmpdir)
            .map(|rd| {
                rd.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("png"))
                    .collect()
            })
            .unwrap_or_default();
        pages.sort();
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
fn ocr_image(path: &Path) -> Result<String, String> {
    let lang = ocr_lang_arg();
    let mut command = ocr_tool_command();
    command.arg(path).arg("-").arg("-l").arg(&lang);
    add_ocr_tessdata_arg(&mut command);
    let out = command
        .output()
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

    let prefix = tmpdir.join("page");
    // pdftoppm -png -r 150 -l <max> <pdf> <prefix> → prefix-1.png, prefix-2.png ...
    let convert = pdf_tool_command("pdftoppm")
        .arg("-png")
        .arg("-r")
        .arg("150")
        .arg("-l")
        .arg(PDF_OCR_MAX_PAGES.to_string())
        .arg(path)
        .arg(&prefix)
        .output();

    let result = match convert {
        Ok(o) if o.status.success() => {
            // 收集生成的 png，按文件名排序保证页序。
            let mut pages: Vec<PathBuf> = std::fs::read_dir(&tmpdir)
                .map(|rd| {
                    rd.filter_map(|e| e.ok().map(|e| e.path()))
                        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("png"))
                        .collect()
                })
                .unwrap_or_default();
            pages.sort();

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
                for (idx, page) in pages.iter().enumerate() {
                    match ocr_image(page) {
                        Ok(text) if !text.trim().is_empty() => {
                            parts.push(format!("## 第 {} 页\n\n{}", idx + 1, text.trim()));
                        }
                        _ => {}
                    }
                }
                let mut content = parts.join("\n\n");
                if pages.len() as u32 >= PDF_OCR_MAX_PAGES {
                    content.push_str(&format!(
                        "\n\n> ⚠️ 扫描件页数较多，OCR 仅处理前 {PDF_OCR_MAX_PAGES} 页"
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
        Ok(o) => IngestResult::warning(
            "pdf",
            &basename,
            result_path,
            byte_size,
            format!(
                "pdftoppm 转图失败: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ),
        ),
        Err(e) => IngestResult::warning(
            "pdf",
            &basename,
            result_path,
            byte_size,
            format!("pdftoppm 调用失败: {e}"),
        ),
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
