//! XLSX-only structural metadata extraction.
//!
//! Calamine intentionally exposes cell values rather than presentation styles.
//! Some spreadsheets encode essential meaning in fill colors, formulas, or
//! merged regions, so retain those OOXML facts as compact text annotations.

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use std::collections::{BTreeMap, HashMap};
use std::io::{Cursor, Read, Seek};

const MAX_STYLE_CELLS: usize = 4_096;
const MAX_FORMULAS: usize = 2_048;
const MAX_MERGED_RANGES: usize = 1_024;
const MAX_XML_ENTRY_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Default)]
struct Fill {
    pattern: Option<String>,
    foreground: Option<String>,
    background: Option<String>,
}

impl Fill {
    fn label(&self) -> Option<String> {
        let pattern = self.pattern.as_deref().unwrap_or("none");
        if pattern == "none" && self.foreground.is_none() && self.background.is_none() {
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
        Some(parts.join(", "))
    }
}

#[derive(Debug, Default)]
struct SheetAnnotations {
    fills: BTreeMap<String, Vec<String>>,
    formulas: Vec<(String, String)>,
    merged_ranges: Vec<String>,
    style_cells_truncated: bool,
    formulas_truncated: bool,
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
        if annotations.fills.is_empty()
            && annotations.formulas.is_empty()
            && annotations.merged_ranges.is_empty()
        {
            continue;
        }
        rendered.push_str("### 工作表结构：");
        rendered.push_str(&sheet_name);
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
        if !annotations.formulas.is_empty() {
            rendered.push_str("- 公式:\n");
            for (cell, formula) in annotations.formulas {
                rendered.push_str("  - ");
                rendered.push_str(&cell);
                rendered.push_str(": =");
                rendered.push_str(&formula);
                rendered.push('\n');
            }
        }
        if annotations.formulas_truncated {
            rendered.push_str("  - 公式过多，以上列表已截断\n");
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
        let normalized = rgb.trim_start_matches('#');
        let rgb = if normalized.len() == 8 {
            &normalized[2..]
        } else {
            normalized
        };
        return Some(format!("#{}", rgb.to_ascii_uppercase()));
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
                b"patternFill" if current_fill.is_some() => {
                    current_fill.as_mut().unwrap().pattern = attr(&element, b"patternType");
                }
                b"fgColor" if current_fill.is_some() => {
                    current_fill.as_mut().unwrap().foreground = color(&element);
                }
                b"bgColor" if current_fill.is_some() => {
                    current_fill.as_mut().unwrap().background = color(&element);
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
                b"patternFill" if current_fill.is_some() => {
                    current_fill.as_mut().unwrap().pattern = attr(&element, b"patternType");
                }
                b"fgColor" if current_fill.is_some() => {
                    current_fill.as_mut().unwrap().foreground = color(&element);
                }
                b"bgColor" if current_fill.is_some() => {
                    current_fill.as_mut().unwrap().background = color(&element);
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
    let mut current_cell: Option<String> = None;
    let mut current_formula = String::new();
    let mut in_formula = false;
    let mut style_cell_count = 0usize;

    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) if element.local_name().as_ref() == b"c" => {
                current_cell = attr(&element, b"r");
                if let (Some(cell), Some(style_id)) = (
                    current_cell.as_ref(),
                    attr(&element, b"s").and_then(|value| value.parse::<usize>().ok()),
                ) {
                    if let Some(Some(fill)) = style_fills.get(style_id) {
                        if style_cell_count < MAX_STYLE_CELLS {
                            annotations
                                .fills
                                .entry(fill.clone())
                                .or_default()
                                .push(cell.clone());
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
                                .push(cell);
                            style_cell_count += 1;
                        } else {
                            annotations.style_cells_truncated = true;
                        }
                    }
                }
            }
            Ok(Event::Start(element)) if element.local_name().as_ref() == b"f" => {
                in_formula = true;
                current_formula.clear();
            }
            Ok(Event::Text(text)) if in_formula => {
                let decoded = text
                    .xml_content(XmlVersion::Implicit1_0)
                    .map_err(|error| format!("解析 XLSX 公式失败: {error}"))?;
                current_formula.push_str(&decoded);
            }
            Ok(Event::GeneralRef(reference)) if in_formula => {
                if let Some(character) = reference
                    .resolve_char_ref()
                    .map_err(|error| format!("解析 XLSX 公式字符引用失败: {error}"))?
                {
                    current_formula.push(character);
                } else {
                    let name = reference
                        .decode()
                        .map_err(|error| format!("解析 XLSX 公式实体失败: {error}"))?;
                    if let Some(value) = quick_xml::escape::resolve_predefined_entity(&name) {
                        current_formula.push_str(value);
                    } else {
                        current_formula.push('&');
                        current_formula.push_str(&name);
                        current_formula.push(';');
                    }
                }
            }
            Ok(Event::CData(text)) if in_formula => {
                let decoded = text
                    .xml_content(XmlVersion::Implicit1_0)
                    .map_err(|error| format!("解析 XLSX 公式 CDATA 失败: {error}"))?;
                current_formula.push_str(&decoded);
            }
            Ok(Event::End(element)) if element.local_name().as_ref() == b"f" => {
                in_formula = false;
                if !current_formula.is_empty() {
                    if annotations.formulas.len() < MAX_FORMULAS {
                        annotations.formulas.push((
                            current_cell.clone().unwrap_or_else(|| "?".into()),
                            current_formula.clone(),
                        ));
                    } else {
                        annotations.formulas_truncated = true;
                    }
                }
            }
            Ok(Event::End(element)) if element.local_name().as_ref() == b"c" => {
                current_cell = None;
                current_formula.clear();
                in_formula = false;
            }
            Ok(Event::Start(element) | Event::Empty(element))
                if element.local_name().as_ref() == b"mergeCell" =>
            {
                if let Some(range) = attr(&element, b"ref") {
                    if annotations.merged_ranges.len() < MAX_MERGED_RANGES {
                        annotations.merged_ranges.push(range);
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
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn synthetic_xlsx() -> Vec<u8> {
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
                    r#"<?xml version="1.0"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fills count="3"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill><fill><patternFill patternType="solid"><fgColor rgb="FF00FF00"/><bgColor indexed="64"/></patternFill></fill></fills><cellXfs count="2"><xf numFmtId="0" fillId="0"/><xf numFmtId="0" fillId="2" applyFill="1"/></cellXfs></styleSheet>"#,
                ),
                (
                    "xl/worksheets/sheet1.xml",
                    r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" s="1" t="inlineStr"><is><t>START</t></is></c></row><row r="2"><c r="C2" s="1"/></row><row r="3"><c r="D3"><f>SUM(A1:B1)</f><v>3</v></c></row></sheetData><mergeCells count="1"><mergeCell ref="A1:B1"/></mergeCells></worksheet>"#,
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

    #[test]
    fn preserves_fills_formulas_and_merged_ranges() {
        let rendered = xlsx_structure_annotations(&synthetic_xlsx())
            .unwrap()
            .unwrap();
        assert!(rendered.contains("### 工作表结构：Map"));
        assert!(rendered.contains("fill #00FF00: A1, C2"));
        assert!(rendered.contains("D3: =SUM(A1:B1)"));
        assert!(rendered.contains("合并区域: A1:B1"));
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
    fn preserves_formulas_and_merges_when_styles_are_absent() {
        let annotations = parse_sheet_annotations(
            r#"<worksheet><sheetData><row><c r="B2"><f>A1&amp;"x"</f></c></row></sheetData><mergeCells><mergeCell ref="C3:D4"/></mergeCells></worksheet>"#,
            &[],
        )
        .unwrap();

        assert_eq!(annotations.formulas, vec![("B2".into(), "A1&\"x\"".into())]);
        assert_eq!(annotations.merged_ranges, vec!["C3:D4"]);
    }
}
