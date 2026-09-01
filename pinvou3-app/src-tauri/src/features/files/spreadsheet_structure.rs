//! XLSX-only structural metadata extraction.
//!
//! Calamine intentionally exposes cell values rather than presentation styles.
//! Some spreadsheets encode essential meaning in fill colors or merged
//! regions, so retain those OOXML facts as compact text annotations. Formula
//! extraction stays on calamine so shared formulas are expanded correctly.

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use std::collections::{BTreeMap, HashMap};
use std::io::{Cursor, Read, Seek};

const MAX_STYLE_CELLS: usize = 4_096;
const MAX_MERGED_RANGES: usize = 1_024;
const MAX_XML_ENTRY_BYTES: u64 = 32 * 1024 * 1024;

/// 上限单个工作表公式格张成面积。calamine 的 `worksheet_formula` 会按公式格
/// `r` 属性极值把 `Range<String>` 物化成稠密网格（`Range::from_sparse`），
/// 恶意文件两个对角公式格即可触发 PB 级单次分配并 abort 宿主进程，必须在
/// 调用前按内容预扫跨度。
pub(super) const MAX_FORMULA_SPAN_CELLS: u64 = 4 * 1024 * 1024;

#[derive(Debug, Default)]
struct Fill {
    pattern: Option<String>,
    foreground: Option<String>,
    background: Option<String>,
}

impl Fill {
    fn label(&self) -> Option<String> {
        let pattern = self.pattern.as_deref().unwrap_or("none");
        if pattern == "none" {
            return None;
        }
        let mut parts = Vec::new();
        if let Some(color) = &self.foreground {
            parts.push(format!("fill {color}"));
        } else if let Some(color) = &self.background {
            parts.push(format!("background {color}"));
        } else {
            parts.push(format!("fill pattern {pattern}"));
        }
        if pattern != "none" && pattern != "solid" {
            parts.push(format!("pattern {pattern}"));
        }
        Some(flatten_text(&parts.join(", ")))
    }
}

#[derive(Debug, Default)]
struct SheetAnnotations {
    fills: BTreeMap<String, Vec<String>>,
    merged_ranges: Vec<String>,
    style_cells_truncated: bool,
    merges_truncated: bool,
}

pub(super) fn xlsx_structure_annotations(bytes: &[u8]) -> Result<Option<String>, String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| format!("打开 XLSX 结构失败: {error}"))?;
    let style_fills = read_zip_text(&mut archive, "xl/styles.xml")?
        .map(|styles| parse_style_fills(&styles))
        .transpose()?
        .unwrap_or_default();

    let workbook = read_zip_text(&mut archive, "xl/workbook.xml")?
        .ok_or_else(|| "XLSX 缺少 workbook.xml".to_string())?;
    let relationships = read_zip_text(&mut archive, "xl/_rels/workbook.xml.rels")?
        .ok_or_else(|| "XLSX 缺少 workbook relationships".to_string())?;
    let relationship_targets = parse_relationship_targets(&relationships)?;
    let sheets = parse_sheets(&workbook)?;

    let mut rendered = String::new();
    for (sheet_name, relationship_id) in sheets {
        let Some(target) = relationship_targets.get(&relationship_id) else {
            continue;
        };
        let Some(path) = normalized_sheet_path(target) else {
            continue;
        };
        let Some(xml) = read_zip_text(&mut archive, &path)? else {
            continue;
        };
        let annotations = parse_sheet_annotations(&xml, &style_fills)?;
        if annotations.fills.is_empty() && annotations.merged_ranges.is_empty() {
            continue;
        }
        rendered.push_str("### 工作表结构：");
        rendered.push_str(&flatten_text(&sheet_name));
        rendered.push('\n');
        for (fill, cells) in annotations.fills {
            rendered.push_str("- ");
            rendered.push_str(&fill);
            rendered.push_str(": ");
            rendered.push_str(&cells.join(", "));
            rendered.push('\n');
        }
        if annotations.style_cells_truncated {
            rendered.push_str("- 填充单元格过多，以上列表已截断\n");
        }
        if !annotations.merged_ranges.is_empty() {
            rendered.push_str("- 合并区域: ");
            rendered.push_str(&annotations.merged_ranges.join(", "));
            rendered.push('\n');
        }
        if annotations.merges_truncated {
            rendered.push_str("- 合并区域过多，以上列表已截断\n");
        }
        rendered.push('\n');
    }

    Ok((!rendered.is_empty()).then_some(rendered))
}

/// 每表公式格跨度 `(rows, cols)`（按表名索引）。`None` 表示 bytes 不是
/// OOXML 工作簿：calamine 对 XLS/ODS 的公式 Range 是打开时预构建、网格有界，
/// 无需守卫。工作表路径解析完全镜像 calamine 自身逻辑（基目录取自根
/// `_rels/.rels` 的 officeDocument Target），非 `xl/` 布局无法绕过守卫。
pub(super) fn xlsx_formula_span_limits(bytes: &[u8]) -> Option<HashMap<String, (u64, u64)>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    let base = workbook_base_folder(&mut archive).ok()?;
    let workbook = read_zip_text(&mut archive, &format!("{base}workbook.xml")).ok()??;
    let relationships =
        read_zip_text(&mut archive, &format!("{base}_rels/workbook.xml.rels")).ok()??;
    let relationship_targets = parse_relationship_targets(&relationships).ok()?;
    let mut limits = HashMap::new();
    for (sheet_name, relationship_id) in parse_sheets(&workbook).ok()? {
        let Some(target) = relationship_targets.get(&relationship_id) else {
            continue;
        };
        let path = if let Some(absolute) = target.strip_prefix('/') {
            absolute.to_string()
        } else {
            format!("{base}{target}")
        };
        let Ok(Some(xml)) = read_zip_text(&mut archive, &path) else {
            continue;
        };
        let (rows, cols) = formula_cell_span(&xml);
        limits.insert(flatten_text(&sheet_name), (rows, cols));
    }
    Some(limits)
}

/// calamine `Xlsx::read_package_relationships` 的镜像：workbook 基目录取自根
/// `_rels/.rels` 中 officeDocument 关系 Target 的目录前缀（剥离开头 `/`）。
fn workbook_base_folder<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> Result<String, String> {
    let root_rels = read_zip_text(archive, "_rels/.rels")?.ok_or("_rels/.rels 缺失")?;
    let mut reader = Reader::from_str(&root_rels);
    reader.config_mut().trim_text(true);
    let mut target = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(element) | Event::Empty(element))
                if element.local_name().as_ref() == b"Relationship" =>
            {
                let is_office_document = attr(&element, b"Type")
                    .is_some_and(|relationship_type| relationship_type.ends_with("officeDocument"));
                if is_office_document {
                    target = attr(&element, b"Target");
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(format!("解析 XLSX 根 relationships 失败: {error}")),
            _ => {}
        }
    }
    let directory = target
        .as_deref()
        .and_then(|target| target.rfind('/').map(|end| &target[..=end]))
        .map(|directory| directory.strip_prefix('/').unwrap_or(directory));
    Ok(directory.unwrap_or("").to_string())
}

/// 工作表内公式格的跨度 `(rows, cols)`，取自带 `r` 属性且含 `<f>` 子元素的
/// 单元格极值。无 `r` 属性的单元格在 calamine 中按顺序递推定位，跨度受 XML
/// 条目体积约束，无需纳入守卫。
fn formula_cell_span(xml: &str) -> (u64, u64) {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut max_row = 0u64;
    let mut max_col = 0u64;
    let mut current_cell: Option<(u64, u64)> = None;
    let mut formula_cell: Option<(u64, u64)> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) => match element.local_name().as_ref() {
                b"c" => {
                    current_cell =
                        attr(&element, b"r").and_then(|reference| parse_cell_reference(&reference));
                }
                b"f" => formula_cell = current_cell,
                _ => {}
            },
            Ok(Event::Empty(element)) if element.local_name().as_ref() == b"f" => {
                formula_cell = current_cell;
            }
            Ok(Event::End(element)) if element.local_name().as_ref() == b"c" => {
                if let Some((row, col)) = formula_cell.take() {
                    max_row = max_row.max(row);
                    max_col = max_col.max(col);
                }
                current_cell = None;
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    (max_row.saturating_add(1), max_col.saturating_add(1))
}

/// A1 风格引用 → 零基 `(row, col)`；畸形输入返回 `None`。
fn parse_cell_reference(reference: &str) -> Option<(u64, u64)> {
    let bytes = reference.as_bytes();
    let mut columns = 0u64;
    let mut digits = 0usize;
    while digits < bytes.len() && bytes[digits].is_ascii_alphabetic() {
        columns = columns
            .checked_mul(26)?
            .checked_add(u64::from(bytes[digits].to_ascii_uppercase() - b'A') + 1)?;
        digits += 1;
    }
    if digits == 0 || digits == bytes.len() {
        return None;
    }
    let row: u64 = reference[digits..].parse().ok()?;
    Some((row.checked_sub(1)?, columns.checked_sub(1)?))
}

/// 注记文本会拼进 ingest 正文「一行 = 一条记录」的行网格，控制空白必须与值
/// 路径同样折叠，防止用字符引用（如 `&#10;`）伪造 `###` 节结构。
pub(super) fn flatten_text(value: &str) -> String {
    value.replace(['\n', '\r', '\t'], " ")
}

fn read_zip_text<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    path: &str,
) -> Result<Option<String>, String> {
    let file = match archive.by_name(path) {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(error) => return Err(format!("读取 XLSX 条目失败: {error}")),
    };
    let mut text = String::new();
    file.take(MAX_XML_ENTRY_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|error| format!("读取 XLSX XML 失败: {error}"))?;
    if text.len() as u64 > MAX_XML_ENTRY_BYTES {
        return Err(format!(
            "XLSX XML 条目超过 {} MiB 限制",
            MAX_XML_ENTRY_BYTES / 1024 / 1024
        ));
    }
    Ok(Some(text))
}

fn attr(element: &BytesStart<'_>, key: &[u8]) -> Option<String> {
    element
        .attributes()
        .with_checks(false)
        .flatten()
        .find(|attribute| attribute.key.local_name().as_ref() == key)
        .and_then(|attribute| {
            attribute
                .normalized_value(XmlVersion::Implicit1_0)
                .ok()
                .map(|value| value.into_owned())
        })
}

fn color(element: &BytesStart<'_>) -> Option<String> {
    if let Some(rgb) = attr(element, b"rgb") {
        return normalized_rgb(&rgb);
    }
    for (key, label) in [
        (b"indexed".as_slice(), "indexed"),
        (b"theme".as_slice(), "theme"),
        (b"auto".as_slice(), "auto"),
    ] {
        if let Some(value) = attr(element, key) {
            return Some(format!("{label}:{value}"));
        }
    }
    None
}

fn normalized_rgb(value: &str) -> Option<String> {
    let normalized = value.trim_start_matches('#');
    if !matches!(normalized.len(), 6 | 8)
        || !normalized.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    let visible = if normalized.len() == 8 {
        normalized.get(2..)?
    } else {
        normalized
    };
    Some(format!("#{}", visible.to_ascii_uppercase()))
}

/// fill 子元素（patternFill/fgColor/bgColor）应用到当前 fill；不在 `<fill>`
/// 内部时静默忽略（如 dxfs 里的同名词素），无 panic 路径。
fn apply_fill_element(current_fill: &mut Option<Fill>, element: &BytesStart<'_>) {
    let Some(fill) = current_fill.as_mut() else {
        return;
    };
    match element.local_name().as_ref() {
        b"patternFill" => fill.pattern = attr(element, b"patternType"),
        b"fgColor" => fill.foreground = color(element),
        b"bgColor" => fill.background = color(element),
        _ => {}
    }
}

fn parse_style_fills(xml: &str) -> Result<Vec<Option<String>>, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut fills = Vec::new();
    let mut cell_fill_ids = Vec::new();
    let mut current_fill: Option<Fill> = None;
    let mut in_fills = false;
    let mut in_cell_xfs = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) => match element.local_name().as_ref() {
                b"fills" => in_fills = true,
                b"fill" if in_fills => current_fill = Some(Fill::default()),
                b"patternFill" | b"fgColor" | b"bgColor" => {
                    apply_fill_element(&mut current_fill, &element);
                }
                b"cellXfs" => in_cell_xfs = true,
                b"xf" if in_cell_xfs => {
                    cell_fill_ids.push(
                        attr(&element, b"fillId")
                            .and_then(|value| value.parse::<usize>().ok())
                            .unwrap_or(0),
                    );
                }
                _ => {}
            },
            Ok(Event::Empty(element)) => match element.local_name().as_ref() {
                // `<fill/>`（空填充）是合法 OOXML：跳过入列会让后续 fillId
                // 整体错位，静默产出错误的填充标注。
                b"fill" if in_fills => fills.push(current_fill.take().unwrap_or_default()),
                b"patternFill" | b"fgColor" | b"bgColor" => {
                    apply_fill_element(&mut current_fill, &element);
                }
                b"xf" if in_cell_xfs => {
                    cell_fill_ids.push(
                        attr(&element, b"fillId")
                            .and_then(|value| value.parse::<usize>().ok())
                            .unwrap_or(0),
                    );
                }
                _ => {}
            },
            Ok(Event::End(element)) => match element.local_name().as_ref() {
                b"fill" if in_fills => fills.push(current_fill.take().unwrap_or_default()),
                b"fills" => in_fills = false,
                b"cellXfs" => in_cell_xfs = false,
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(error) => return Err(format!("解析 XLSX 样式失败: {error}")),
            _ => {}
        }
    }

    Ok(cell_fill_ids
        .into_iter()
        .map(|fill_id| fills.get(fill_id).and_then(Fill::label))
        .collect())
}

fn parse_relationship_targets(xml: &str) -> Result<HashMap<String, String>, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut targets = HashMap::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(element) | Event::Empty(element))
                if element.local_name().as_ref() == b"Relationship" =>
            {
                if let (Some(id), Some(target)) = (attr(&element, b"Id"), attr(&element, b"Target"))
                {
                    targets.insert(id, target);
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(format!("解析 XLSX relationships 失败: {error}")),
            _ => {}
        }
    }
    Ok(targets)
}

fn parse_sheets(xml: &str) -> Result<Vec<(String, String)>, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut sheets = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(element) | Event::Empty(element))
                if element.local_name().as_ref() == b"sheet" =>
            {
                if let (Some(name), Some(id)) = (attr(&element, b"name"), attr(&element, b"id")) {
                    sheets.push((name, id));
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(format!("解析 XLSX 工作表失败: {error}")),
            _ => {}
        }
    }
    Ok(sheets)
}

fn normalized_sheet_path(target: &str) -> Option<String> {
    let target = target.trim_start_matches('/');
    let candidate = if target.starts_with("xl/") {
        target.to_string()
    } else {
        format!("xl/{target}")
    };
    (!candidate.split('/').any(|part| part == "..") && candidate.starts_with("xl/"))
        .then_some(candidate)
}

fn parse_sheet_annotations(
    xml: &str,
    style_fills: &[Option<String>],
) -> Result<SheetAnnotations, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut annotations = SheetAnnotations::default();
    let mut style_cell_count = 0usize;

    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) if element.local_name().as_ref() == b"c" => {
                if let (Some(cell), Some(style_id)) = (
                    attr(&element, b"r"),
                    attr(&element, b"s").and_then(|value| value.parse::<usize>().ok()),
                ) {
                    if let Some(Some(fill)) = style_fills.get(style_id) {
                        if style_cell_count < MAX_STYLE_CELLS {
                            annotations
                                .fills
                                .entry(fill.clone())
                                .or_default()
                                .push(flatten_text(&cell));
                            style_cell_count += 1;
                        } else {
                            annotations.style_cells_truncated = true;
                        }
                    }
                }
            }
            Ok(Event::Empty(element)) if element.local_name().as_ref() == b"c" => {
                if let (Some(cell), Some(style_id)) = (
                    attr(&element, b"r"),
                    attr(&element, b"s").and_then(|value| value.parse::<usize>().ok()),
                ) {
                    if let Some(Some(fill)) = style_fills.get(style_id) {
                        if style_cell_count < MAX_STYLE_CELLS {
                            annotations
                                .fills
                                .entry(fill.clone())
                                .or_default()
                                .push(flatten_text(&cell));
                            style_cell_count += 1;
                        } else {
                            annotations.style_cells_truncated = true;
                        }
                    }
                }
            }
            Ok(Event::Start(element) | Event::Empty(element))
                if element.local_name().as_ref() == b"mergeCell" =>
            {
                if let Some(range) = attr(&element, b"ref") {
                    if annotations.merged_ranges.len() < MAX_MERGED_RANGES {
                        annotations.merged_ranges.push(flatten_text(&range));
                    } else {
                        annotations.merges_truncated = true;
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(format!("解析 XLSX 工作表结构失败: {error}")),
            _ => {}
        }
    }
    Ok(annotations)
}

#[cfg(test)]
pub(super) fn synthetic_xlsx_fixture() -> Vec<u8> {
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    let mut bytes = Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut bytes);
        let options = SimpleFileOptions::default();
        let files = [
            (
                "[Content_Types].xml",
                r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/></Types>"#,
            ),
            (
                "_rels/.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
            ),
            (
                "xl/workbook.xml",
                r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Map" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#,
            ),
            (
                "xl/styles.xml",
                r#"<?xml version="1.0"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fills count="3"><fill><patternFill patternType="none"><fgColor rgb="FFFF0000"/></patternFill></fill><fill><patternFill patternType="gray125"/></fill><fill><patternFill patternType="solid"><fgColor rgb="FF00FF00"/><bgColor indexed="64"/></patternFill></fill></fills><cellXfs count="2"><xf numFmtId="0" fillId="0"/><xf numFmtId="0" fillId="2" applyFill="1"/></cellXfs></styleSheet>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><dimension ref="A1:D4"/><sheetData><row r="1"><c r="A1" s="1" t="inlineStr"><is><t>START</t></is></c></row><row r="2"><c r="C2" s="1"/></row><row r="3"><c r="D3"><f t="shared" ref="D3:D4" si="0">SUM(A1:B1)</f><v>3</v></c></row><row r="4"><c r="D4"><f t="shared" si="0"/><v>5</v></c></row></sheetData><mergeCells count="1"><mergeCell ref="A1:B1"/></mergeCells></worksheet>"#,
            ),
        ];
        for (path, content) in files {
            zip.start_file(path, options).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }
    bytes.into_inner()
}

#[cfg(test)]
fn xlsx_fixture(files: &[(&str, &str)]) -> Vec<u8> {
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    let mut bytes = Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut bytes);
        let options = SimpleFileOptions::default();
        for (path, content) in files {
            zip.start_file(path, options).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }
    bytes.into_inner()
}

const TEST_CONTENT_TYPES: &str = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/></Types>"#;

#[cfg(test)]
pub(super) fn distant_formula_fixture() -> Vec<u8> {
    xlsx_fixture(&[
        ("[Content_Types].xml", TEST_CONTENT_TYPES),
        (
            "_rels/.rels",
            r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
        ),
        (
            "xl/workbook.xml",
            r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Map" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
        ),
        (
            "xl/_rels/workbook.xml.rels",
            r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
        ),
        // 值全部聚在 A1，两个对角公式格把公式格张成撑到整表极限：
        // calamine 的稠密物化会请求 2^32 × 16384 × 24 字节，必须被守卫拦截。
        (
            "xl/worksheets/sheet1.xml",
            r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><f>SUM(1)</f><v>1</v></c></row><row r="4294967295"><c r="XFD4294967295"><f>SUM(1)</f></c></row></sheetData></worksheet>"#,
        ),
    ])
}

#[cfg(test)]
pub(super) fn folded_formula_fixture() -> Vec<u8> {
    xlsx_fixture(&[
        ("[Content_Types].xml", TEST_CONTENT_TYPES),
        (
            "_rels/.rels",
            r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
        ),
        (
            "xl/workbook.xml",
            r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Fold&#10;X" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
        ),
        (
            "xl/_rels/workbook.xml.rels",
            r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
        ),
        (
            "xl/worksheets/sheet1.xml",
            r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><f>SUM(1)&#10;+ 2</f><v>3</v></c></row></sheetData></worksheet>"#,
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_fills_and_merged_ranges() {
        let rendered = xlsx_structure_annotations(&synthetic_xlsx_fixture())
            .unwrap()
            .unwrap();
        assert!(rendered.contains("### 工作表结构：Map"));
        assert!(rendered.contains("fill #00FF00: A1, C2"));
        assert!(rendered.contains("合并区域: A1:B1"));
        assert!(!rendered.contains("#FF0000"));
    }

    #[test]
    fn rejects_relationship_path_traversal() {
        assert_eq!(normalized_sheet_path("../secrets.xml"), None);
        assert_eq!(
            normalized_sheet_path("worksheets/sheet1.xml").as_deref(),
            Some("xl/worksheets/sheet1.xml")
        );
    }

    #[test]
    fn preserves_merges_when_styles_are_absent() {
        let annotations = parse_sheet_annotations(
            r#"<worksheet><sheetData><row><c r="B2"><f>A1&amp;"x"</f></c></row></sheetData><mergeCells><mergeCell ref="C3:D4"/></mergeCells></worksheet>"#,
            &[],
        )
        .unwrap();

        assert_eq!(annotations.merged_ranges, vec!["C3:D4"]);
    }

    #[test]
    fn rejects_non_ascii_or_malformed_rgb_without_panicking() {
        assert_eq!(normalized_rgb("FéFF00F"), None);
        assert_eq!(normalized_rgb("GG00FF00"), None);
        assert_eq!(normalized_rgb("FF00ff00").as_deref(), Some("#00FF00"));
    }

    #[test]
    fn none_pattern_ignores_decorative_color_attributes() {
        assert_eq!(
            Fill {
                pattern: Some("none".into()),
                foreground: Some("#FF0000".into()),
                background: None,
            }
            .label(),
            None
        );
    }

    #[test]
    fn formula_span_limits_track_formula_cells_only() {
        let limits = xlsx_formula_span_limits(&synthetic_xlsx_fixture()).unwrap();
        // 只有 D3/D4 带公式；值格 A1/C2 不参与张成。
        assert_eq!(limits.get("Map"), Some(&(4, 4)));
    }

    #[test]
    fn non_ooxml_bytes_have_no_formula_span_limits() {
        assert!(xlsx_formula_span_limits(b"not a zip").is_none());
    }

    #[test]
    fn distant_formula_cells_exceed_span_limit() {
        let limits = xlsx_formula_span_limits(&distant_formula_fixture()).unwrap();
        let &(rows, cols) = limits.get("Map").expect("恶意 fixture 应解析出 Map");
        assert_eq!((rows, cols), (4_294_967_295, 16_384));
        assert!(rows.saturating_mul(cols) > MAX_FORMULA_SPAN_CELLS);
    }

    #[test]
    fn guard_resolves_nonstandard_base_folder() {
        let bytes = xlsx_fixture(&[
            ("[Content_Types].xml", TEST_CONTENT_TYPES),
            (
                "_rels/.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="deep/workbook.xml"/></Relationships>"#,
            ),
            (
                "deep/workbook.xml",
                r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Base" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
            ),
            (
                "deep/_rels/workbook.xml.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
            ),
            (
                "deep/worksheets/sheet1.xml",
                r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="2"><c r="B2"><f>A1</f></c></row></sheetData></worksheet>"#,
            ),
        ]);
        let limits = xlsx_formula_span_limits(&bytes).unwrap();
        assert_eq!(limits.get("Base"), Some(&(2, 2)));
    }

    #[test]
    fn self_closing_fill_keeps_style_indices() {
        let bytes = xlsx_fixture(&[
            ("[Content_Types].xml", TEST_CONTENT_TYPES),
            (
                "_rels/.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
            ),
            (
                "xl/workbook.xml",
                r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Map" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
            ),
            (
                "xl/styles.xml",
                r#"<?xml version="1.0"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fills count="2"><fill/><fill><patternFill patternType="solid"><fgColor rgb="FF00FF00"/></patternFill></fill></fills><cellXfs count="1"><xf numFmtId="0" fillId="1" applyFill="1"/></cellXfs></styleSheet>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="2"><c r="B2" s="0"/></row></sheetData></worksheet>"#,
            ),
        ]);
        let rendered = xlsx_structure_annotations(&bytes).unwrap().unwrap();
        // `<fill/>` 占据 fillId 0：跳过它会让 fillId=1 越界、注记整个丢失。
        assert!(rendered.contains("fill #00FF00: B2"), "{rendered}");
    }

    #[test]
    fn annotation_output_folds_control_whitespace() {
        let bytes = xlsx_fixture(&[
            ("[Content_Types].xml", TEST_CONTENT_TYPES),
            (
                "_rels/.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
            ),
            (
                "xl/workbook.xml",
                r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Bad&#10;### 工作表结构：X" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
            ),
            (
                "xl/styles.xml",
                r#"<?xml version="1.0"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fills count="1"><fill><patternFill patternType="solid"><fgColor rgb="FF00FF00"/></patternFill></fill></fills><cellXfs count="1"><xf numFmtId="0" fillId="0" applyFill="1"/></cellXfs></styleSheet>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A&#10;1" s="0"/></row></sheetData><mergeCells count="1"><mergeCell ref="C&#10;3:D4"/></mergeCells></worksheet>"#,
            ),
        ]);
        let rendered = xlsx_structure_annotations(&bytes).unwrap().unwrap();
        assert!(
            rendered.contains("### 工作表结构：Bad ### 工作表结构：X"),
            "{rendered}"
        );
        assert!(rendered.contains("fill #00FF00: A 1"), "{rendered}");
        assert!(rendered.contains("合并区域: C 3:D4"), "{rendered}");
        // 折叠后只允许出现一个节头，伪造的 `###` 标题必须消失。
        assert_eq!(
            rendered
                .lines()
                .filter(|line| line.starts_with("### 工作表结构"))
                .count(),
            1
        );
    }

    #[test]
    fn fill_label_folds_control_whitespace() {
        let label = Fill {
            pattern: Some("solid".into()),
            foreground: None,
            background: Some("indexed:6\n4".into()),
        }
        .label()
        .unwrap();
        assert_eq!(label, "background indexed:6 4");
    }
}
