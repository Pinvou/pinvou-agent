//! Office 系格式摄入：pandoc 文字文档 + LibreOffice 旧格式 / 表格 / 演示。
//!
//! 涵盖四类 Office 分派：
//! - `doc_pandoc`（docx/odt）：pandoc 原生转 markdown；
//! - `doc_office`（doc/rtf/wps）：LibreOffice headless 转纯文本；
//! - `spreadsheet`（xlsx/ods/xls/et）：calamine 纯 Rust 读全部工作表，失败回退 LibreOffice CSV；
//! - `presentation`（pptx/ppt/odp/dps）：LibreOffice 转 PDF → pdftotext。
//!
//! LibreOffice 调用统一用独立 UserInstallation profile（防并发 lock）+ 唯一临时目录。

use std::path::Path;

use super::IngestResult;
use super::estimate_tokens;
use super::ingest_deps::{
    libreoffice_tool_command, libreoffice_user_installation_arg, pandoc_tool_command,
    pdf_tool_command, system_tools,
};

const MAX_FORMULAS_PER_SHEET: usize = 2_048;

/// pandoc 原生支持的文字文档（docx/odt）摄入：`pandoc -t markdown`。
pub(super) fn ingest_pandoc(
    path: &Path,
    basename: String,
    byte_size: u64,
    label: &str,
) -> IngestResult {
    let tools = system_tools();
    if !tools.pandoc {
        return IngestResult::warning(
            label,
            &basename,
            path,
            byte_size,
            crate::platform::os::pandoc_missing_message(),
        );
    }
    let out = pandoc_tool_command()
        .arg("-t")
        .arg("markdown")
        .arg(path)
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let content = String::from_utf8_lossy(&o.stdout).into_owned();
            IngestResult::with_markdown(label, &basename, path, byte_size, content)
        }
        Ok(o) => IngestResult::warning(
            label,
            &basename,
            path,
            byte_size,
            format!("pandoc 失败: {}", String::from_utf8_lossy(&o.stderr).trim()),
        ),
        Err(e) => IngestResult::warning(
            label,
            &basename,
            path,
            byte_size,
            format!("pandoc 调用失败: {e}"),
        ),
    }
}

/// 用 LibreOffice headless 把文件转成指定 filter 的产物并读回文本。复用于旧
/// office、WPS 文字、电子表格等 pandoc 吃不下的格式。`convert_to` 是 soffice 的
/// 输出 filter 串，`out_ext` 是产物扩展名（用来在临时目录里定位输出文件）。
fn libreoffice_convert_text(
    path: &Path,
    convert_to: &str,
    out_ext: &str,
) -> Result<String, String> {
    if !system_tools().libreoffice {
        return Err(crate::platform::os::libreoffice_missing_message().into());
    }
    // 临时目录：每次唯一，避免并发文件名冲突。
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmpdir = std::env::temp_dir().join(format!("pinvou3-libreoffice-{ts}"));
    std::fs::create_dir_all(&tmpdir).map_err(|e| format!("创建临时目录失败: {e}"))?;

    // 独立 UserInstallation profile：LibreOffice 同一 profile 不能并发(会 lock)，
    // 用户一次拖多个 office 文件时前端会并发 ingest_file，必须各用各的 profile。
    let out = libreoffice_tool_command()
        .arg(libreoffice_user_installation_arg(&tmpdir.join("profile"))?)
        .arg("--headless")
        .arg("--convert-to")
        .arg(convert_to)
        .arg("--outdir")
        .arg(&tmpdir)
        .arg(path)
        .output();

    let result = match out {
        Ok(o) if o.status.success() => {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("converted");
            let out_path = tmpdir.join(format!("{stem}.{out_ext}"));
            std::fs::read_to_string(&out_path)
                // soffice 的 txt/csv 导出会带 UTF-8 BOM，去掉以免污染正文开头。
                .map(|s| s.trim_start_matches('\u{feff}').to_string())
                .map_err(|e| format!("LibreOffice 转换后读取失败: {e}"))
        }
        Ok(o) => Err(format!(
            "LibreOffice 转换失败: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(e) => Err(format!("LibreOffice 调用失败: {e}")),
    };
    let _ = std::fs::remove_dir_all(&tmpdir);
    result
}

/// 文字类文档（.doc/.rtf + WPS .wps）：pandoc 吃不下，用 LibreOffice 转纯文本。
pub(super) fn ingest_office_text(
    path: &Path,
    basename: String,
    byte_size: u64,
    kind: &str,
) -> IngestResult {
    match libreoffice_convert_text(path, "txt:Text (encoded):UTF8", "txt") {
        Ok(content) => IngestResult::with_markdown(kind, &basename, path, byte_size, content),
        Err(e) => IngestResult::warning(kind, &basename, path, byte_size, e),
    }
}

/// 电子表格（.xlsx/.ods/.xls + WPS .et）：用 calamine 纯 Rust 读**全部工作表**逐行抽取。
/// 旧实现走 LibreOffice CSV 只导「活动工作表」，多 sheet 文件丢 90% 内容（实测 4 sheet
/// 散热报告只抽到首页 418 字、CPU/温升数据全丢）。calamine 失败时（私有 .et 格式等）
/// 回退 LibreOffice CSV，至少拿到首个工作表，不退化于旧行为。
pub(super) fn ingest_spreadsheet(
    path: &Path,
    basename: String,
    path_str: String,
    byte_size: u64,
    kind: &str,
) -> IngestResult {
    // (markdown, warning)：calamine 成功取全表；失败回退 LibreOffice CSV（仅首个工作表）。
    let (markdown, warning) = match spreadsheet_all_sheets_text(path) {
        Ok(body) if !body.trim().is_empty() => (Some(body), None),
        other => {
            let fb_err = other.err();
            match libreoffice_convert_text(path, "csv:Text - txt - csv (StarCalc)", "csv") {
                Ok(content) => (
                    Some(format!(
                        "> 注：表格解析回退到 CSV，若原文件含多个工作表，此处仅含首个工作表。\n\n{content}"
                    )),
                    None,
                ),
                Err(e) => (None, Some(fb_err.unwrap_or(e))),
            }
        }
    };
    let token_estimate = markdown.as_deref().map(estimate_tokens).unwrap_or(0);
    IngestResult {
        kind: kind.into(),
        basename,
        path: path_str,
        markdown,
        token_estimate,
        byte_size,
        warning,
    }
}

/// calamine 读电子表格全部工作表 → 逐行文本。每个工作表加 `## 工作表：名` 小标题，
/// 每行用 ` | ` 连接非空尾部单元格（去掉行尾连续空单元格），整行空则跳过。
/// 单元格内换行折成空格，避免破坏「一行 = 一条记录」的语义（利于切块/检索）。
fn spreadsheet_all_sheets_text(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读取文件失败: {e}"))?;
    let preserve_xlsx_structure = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("xlsx"));
    spreadsheet_text_from_bytes(&bytes, preserve_xlsx_structure)
}

/// 抽取核心（接收字节，便于单测喂 fixture）。见 [`spreadsheet_all_sheets_text`]。
fn spreadsheet_text_from_bytes(
    bytes: &[u8],
    preserve_xlsx_structure: bool,
) -> Result<String, String> {
    // DataType trait 提供 Data::is_empty()（calamine 0.26 是 trait 方法，非固有）。
    use calamine::{Data, DataType, Reader, open_workbook_auto_from_rs};

    let mut wb = open_workbook_auto_from_rs(std::io::Cursor::new(bytes))
        .map_err(|e| format!("calamine 解析失败: {e}"))?;

    // calamine 对 XLSX 公式 Range 按公式格极值做稠密物化（见
    // spreadsheet_structure::MAX_FORMULA_SPAN_CELLS），必须先按内容预扫跨度；
    // XLS/ODS 是打开时预构建、网格有界，返回 None 即无需守卫。
    let formula_span_limits = super::spreadsheet_structure::xlsx_formula_span_limits(bytes);

    let cell = |c: &Data| -> String {
        if c.is_empty() {
            String::new()
        } else {
            c.to_string()
                .replace(['\n', '\r', '\t'], " ")
                .trim()
                .to_string()
        }
    };

    let mut out = String::new();
    let names: Vec<String> = wb.sheet_names().clone();
    for name in names {
        let range = match wb.worksheet_range(&name) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if range.is_empty() {
            continue;
        }
        out.push_str("## 工作表：");
        out.push_str(&name);
        out.push('\n');
        for row in range.rows() {
            let cells: Vec<String> = row.iter().map(&cell).collect();
            let last = match cells.iter().rposition(|c| !c.is_empty()) {
                Some(i) => i,
                None => continue, // 整行空
            };
            out.push_str(&cells[..=last].join(" | "));
            out.push('\n');
        }
        out.push('\n');
        let formula_span = formula_span_limits
            .as_ref()
            .and_then(|limits| limits.get(&name));
        if formula_span.is_none_or(|&(rows, cols)| {
            rows.saturating_mul(cols) <= super::spreadsheet_structure::MAX_FORMULA_SPAN_CELLS
        }) {
            match wb.worksheet_formula(&name) {
                Ok(formulas) => append_formula_annotations(&mut out, &name, &formulas),
                Err(error) => log::warn!("spreadsheet formula extraction failed: {error}"),
            }
        } else {
            log::warn!(
                "spreadsheet formula extraction skipped for sheet {name}: formula cell span exceeds limit"
            );
        }
    }
    if preserve_xlsx_structure {
        match super::spreadsheet_structure::xlsx_structure_annotations(bytes) {
            Ok(Some(structure)) => out.push_str(&structure),
            Ok(None) => {}
            Err(error) => log::warn!("XLSX structure extraction failed: {error}"),
        }
    }
    Ok(out)
}

fn append_formula_annotations(
    out: &mut String,
    sheet_name: &str,
    formulas: &calamine::Range<String>,
) {
    let Some((start_row, start_column)) = formulas.start() else {
        return;
    };
    let mut written = 0usize;
    let mut truncated = false;
    for (relative_row, relative_column, formula) in formulas.used_cells() {
        if formula.is_empty() {
            continue;
        }
        if written == MAX_FORMULAS_PER_SHEET {
            truncated = true;
            break;
        }
        if written == 0 {
            out.push_str("### 工作表公式：");
            out.push_str(&super::spreadsheet_structure::flatten_text(sheet_name));
            out.push('\n');
        }
        out.push_str("- ");
        out.push_str(&a1_cell(
            start_row.saturating_add(relative_row as u32),
            start_column.saturating_add(relative_column as u32),
        ));
        out.push_str(": =");
        out.push_str(&super::spreadsheet_structure::flatten_text(
            formula.strip_prefix('=').unwrap_or(formula),
        ));
        out.push('\n');
        written += 1;
    }
    if truncated {
        out.push_str("- 公式过多，以上列表已截断\n");
    }
    if written > 0 {
        out.push('\n');
    }
}

fn a1_cell(row: u32, column: u32) -> String {
    let mut remaining = u64::from(column) + 1;
    let mut letters = Vec::new();
    while remaining > 0 {
        let digit = ((remaining - 1) % 26) as u8;
        letters.push(char::from(b'A' + digit));
        remaining = (remaining - 1) / 26;
    }
    letters.reverse();
    let column_name: String = letters.into_iter().collect();
    format!("{column_name}{}", u64::from(row) + 1)
}

/// 演示类（.pptx/.ppt/.odp + WPS .dps）：LibreOffice **没有 Impress→txt 导出**
/// （实测无产出），所以先转 PDF 再用 pdftotext 抽每页文字（标题/要点/中文都保留）。
/// 自己转出的 PDF 必有文字层，不需要扫描件 OCR 兜底。
pub(super) fn ingest_presentation(
    path: &Path,
    basename: String,
    byte_size: u64,
    kind: &str,
) -> IngestResult {
    let tools = system_tools();
    if !tools.libreoffice || !tools.pdftotext {
        return IngestResult::warning(
            kind,
            &basename,
            path,
            byte_size,
            crate::platform::os::presentation_pdf_missing_message(),
        );
    }
    match libreoffice_presentation_text(path) {
        Ok(content) if !content.trim().is_empty() => {
            IngestResult::with_markdown(kind, &basename, path, byte_size, content)
        }
        Ok(_) => IngestResult::warning(kind, &basename, path, byte_size, "演示文稿未提取到文字"),
        Err(e) => IngestResult::warning(kind, &basename, path, byte_size, e),
    }
}

/// 演示文稿 → PDF（LibreOffice）→ pdftotext 的串联，返回纯文本。
fn libreoffice_presentation_text(path: &Path) -> Result<String, String> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmpdir = std::env::temp_dir().join(format!("pinvou3-pptpdf-{ts}"));
    std::fs::create_dir_all(&tmpdir).map_err(|e| format!("创建临时目录失败: {e}"))?;

    let convert = libreoffice_tool_command()
        .arg(libreoffice_user_installation_arg(&tmpdir.join("profile"))?)
        .arg("--headless")
        .arg("--convert-to")
        .arg("pdf")
        .arg("--outdir")
        .arg(&tmpdir)
        .arg(path)
        .output();

    let result = match convert {
        Ok(o) if o.status.success() => {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("converted");
            let pdf_path = tmpdir.join(format!("{stem}.pdf"));
            pdf_tool_command("pdftotext")
                .arg("-layout")
                .arg(&pdf_path)
                .arg("-")
                .output()
                .map_err(|e| format!("pdftotext 调用失败: {e}"))
                .and_then(|o| {
                    if o.status.success() {
                        Ok(String::from_utf8_lossy(&o.stdout).into_owned())
                    } else {
                        Err(format!(
                            "pdftotext 失败: {}",
                            String::from_utf8_lossy(&o.stderr).trim()
                        ))
                    }
                })
        }
        Ok(o) => Err(format!(
            "LibreOffice 转 PDF 失败: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(e) => Err(format!("LibreOffice 调用失败: {e}")),
    };
    let _ = std::fs::remove_dir_all(&tmpdir);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spreadsheet_extracts_all_sheets() {
        // 防回归：旧实现走 LibreOffice CSV 只导「活动工作表」，多 sheet 文件丢 90% 内容
        // （实测 4-sheet 散热报告只抽到首页、CPU/温升数据全失）。calamine 必须把
        // 全部工作表逐行抽出。fixture 是 3-sheet 合成表（Cover/配置表/温升表）。
        let bytes = include_bytes!("../../../test-fixtures/multi_sheet.xlsx");
        let txt = spreadsheet_text_from_bytes(bytes, true).expect("calamine 应能解析 fixture");
        // 三个工作表标题都要在
        assert!(txt.contains("## 工作表：Cover"), "缺 Cover sheet");
        assert!(
            txt.contains("## 工作表：System configuration"),
            "缺配置表 sheet"
        );
        assert!(
            txt.contains("## 工作表：Thermal test result"),
            "缺温升表 sheet"
        );
        // 非首表的关键内容（旧 CSV 实现会整段丢失）
        assert!(txt.contains("Ultra 7 258V"), "应抽到第二表的 CPU 型号");
        assert!(txt.contains("83.6"), "应抽到第三表的温度实测值");
        // 行结构保留：同一行单元格用 ' | ' 连接
        assert!(
            txt.contains("CPU | key part | model | Ultra 7 258V SRPMN"),
            "同一行单元格应保留在一行"
        );
    }

    #[test]
    fn spreadsheet_appends_structure_and_expanded_shared_formulas() {
        let bytes = super::super::spreadsheet_structure::synthetic_xlsx_fixture();
        let txt = spreadsheet_text_from_bytes(&bytes, true).expect("应能解析合成 XLSX");

        assert!(txt.contains("### 工作表公式：Map"));
        assert!(txt.contains("D3: =SUM(A1:B1)"));
        assert!(txt.contains("D4: =SUM(A2:B2)"));
        assert!(txt.contains("### 工作表结构：Map"));
        assert!(txt.contains("fill #00FF00: A1, C2"));
        assert!(txt.contains("合并区域: A1:B1"));
    }

    #[test]
    fn converts_zero_based_coordinates_to_a1_notation() {
        assert_eq!(a1_cell(0, 0), "A1");
        assert_eq!(a1_cell(9, 25), "Z10");
        assert_eq!(a1_cell(0, 26), "AA1");
        assert_eq!(a1_cell(41, 701), "ZZ42");
    }

    #[test]
    fn distant_formula_span_sheet_keeps_values_without_formulas() {
        // 若跨度守卫被移除，calamine 会对该 fixture 触发 PB 级稠密分配并
        // abort 整个测试进程（红=进程崩溃），而非普通断言失败。
        let bytes = super::super::spreadsheet_structure::distant_formula_fixture();
        let txt = spreadsheet_text_from_bytes(&bytes, true).expect("ingest 应安全完成");
        assert!(txt.contains("## 工作表：Map"), "值路径不受守卫影响");
        assert!(
            !txt.contains("### 工作表公式"),
            "超限公式的注记应整段跳过: {txt}"
        );
    }

    #[test]
    fn formula_annotations_fold_control_whitespace() {
        let bytes = super::super::spreadsheet_structure::folded_formula_fixture();
        let txt = spreadsheet_text_from_bytes(&bytes, true).expect("应能解析 fixture");
        assert!(txt.contains("### 工作表公式：Fold X"), "{txt}");
        assert!(txt.contains("A1: =SUM(1) + 2"), "{txt}");
        assert!(!txt.contains("=SUM(1)\n"), "公式文本不得携带原始换行");
    }
}
