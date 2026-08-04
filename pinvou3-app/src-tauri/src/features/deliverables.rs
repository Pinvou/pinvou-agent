//! 成品(交付物)业务逻辑。
//!
//! 从 `app::commands::artifacts` 回流的纯域逻辑:奏折「成品箱」装配、跨会话
//! 「产出物」索引、衙门式文件名 → 题目命名的物化副本,以及旧奏折(无成品清单
//! 段)的成品归属语义推断。Tauri 命令薄壳留在 `app::commands::artifacts`,本
//! 模块只承接装配实现(可读盘、可做语义推断,不持 Tauri 命令属性、不
//! 依赖 `app` 层)。
//!
//! 设计原则(白浪定的):宝箱内容 = 奏折申报的成品,后端不猜;旧 run 没申报才
//! 回退到语义推断 + final_report 副本,不至于空箱。

use serde::{Deserialize, Serialize};

/// 旧奏折(无成品清单段)的成品推断:语义归因,三路信号给六部计分。
/// ① 本部章节(### 标题点名 <bu>_N.md)含成品描述词(整合/最终/成品/完整/汇总);
/// ② **被质检对象**——质检章节(标题含审核/校验/验收)里「对〈X部〉…的」的 X
///   (质检节内的成品词描述被审对象,绝不归审核者——天真就近归因实测翻车);
/// ③ 对账表「交付」达成行点名的部。
/// 返回 (bu_id, 得分);得分 <8(单一信号)不可信,调用方回退。
pub(crate) fn infer_product_bu(report: &str) -> Option<(String, i32)> {
    const BU: &[(&str, &str)] = &[
        ("兵部", "bingbu"),
        ("户部", "hubu"),
        ("礼部", "libu"),
        ("刑部", "xingbu"),
        ("工部", "gongbu"),
        ("吏部", "libu_renshi"),
    ];
    const QA_WORDS: &[&str] = &["审核", "校验", "验收", "质检"];
    const PRODUCT_WORDS: &[(&str, i32)] = &[
        ("整合", 3),
        ("最终", 3),
        ("成品", 4),
        ("完整", 2),
        ("汇总", 2),
    ];
    let mut scores: std::collections::HashMap<&str, i32> = Default::default();
    // 按 markdown 标题分节
    let mut sections: Vec<String> = Vec::new();
    let mut cur = String::new();
    for line in report.lines() {
        if line.starts_with("##") && !cur.is_empty() {
            sections.push(std::mem::take(&mut cur));
        }
        cur.push_str(line);
        cur.push('\n');
    }
    sections.push(cur);
    for sec in &sections {
        let head = sec.lines().next().unwrap_or("");
        if QA_WORDS.iter().any(|w| head.contains(w)) {
            // 质检节:只认「对〈X部〉」归因(中文部名或拼音 id 都认)
            for (cn, en) in BU {
                if sec.contains(&format!("对{cn}")) || sec.contains(&format!("对{en}")) {
                    *scores.entry(en).or_default() += 5;
                }
            }
            continue;
        }
        // 标题点名 <bu>_<数字>(后随数字才算,防 libu_ 误吃 libu_renshi_1.md)
        let names_bu = |head: &str, en: &str| {
            let pat = format!("{en}_");
            head.match_indices(&pat).any(|(i, _)| {
                head[i + pat.len()..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit())
            })
        };
        if let Some((_, en)) = BU.iter().rev().find(|(_, en)| names_bu(head, en)) {
            let pts: i32 = PRODUCT_WORDS
                .iter()
                .filter(|(k, _)| sec.contains(k))
                .map(|(_, w)| w)
                .sum();
            *scores.entry(en).or_default() += pts;
        }
    }
    for line in report.lines() {
        if line.contains("交付") && (line.contains('✅') || line.contains("达成")) {
            for (cn, en) in BU {
                if line.contains(cn) {
                    *scores.entry(en).or_default() += 3;
                }
            }
        }
    }
    scores
        .into_iter()
        .max_by_key(|(_, s)| *s)
        .map(|(b, s)| (b.to_string(), s))
}

/// 题目转安全文件名:去掉路径分隔/非法字符,截长,空了给兜底。
fn sanitize_title_filename(title: &str, fallback: &str) -> String {
    let cleaned: String = title
        .chars()
        .filter(|c| {
            !matches!(
                c,
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\n' | '\r' | '\0'
            )
        })
        .collect::<String>()
        .trim()
        .chars()
        .take(48)
        .collect();
    if cleaned.is_empty() {
        fallback.to_string()
    } else {
        cleaned
    }
}

/// 从结案呈报解析「成品清单/工件清单」段申报的成品相对路径。
/// 与引擎 validate_deliverable.check_artifact_manifest 同一约定:
/// 标题段内的列表项,路径写在反引号里(无反引号则取首个空白分隔 token),
/// 到下一个标题为止。
fn parse_product_manifest(report_text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_section = false;
    for line in report_text.lines() {
        let t = line.trim_start();
        if t.starts_with('#') {
            let h = t.trim_start_matches('#').trim();
            in_section = h.starts_with("成品清单") || h.starts_with("工件清单");
            continue;
        }
        if !in_section {
            continue;
        }
        let s = line.trim();
        if !(s.starts_with('-') || s.starts_with('*')) {
            continue;
        }
        let item = s.trim_start_matches(['-', '*', ' ']).trim();
        let rel = if let Some(a) = item.find('`') {
            item[a + 1..].split('`').next().unwrap_or("")
        } else {
            item.split_whitespace().next().unwrap_or("")
        };
        if !rel.is_empty() {
            out.push(rel.to_string());
        }
    }
    out
}

/// md 首个一级/二级标题做客户可读的展示标题(客户看不懂 libu_1.md 这种衙门名)。
fn md_display_title(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(8192)]);
    text.lines().find_map(|l| {
        let t = l.trim_start();
        t.strip_prefix("## ")
            .or_else(|| t.strip_prefix("# "))
            .map(|h| h.trim().chars().take(60).collect::<String>())
    })
}

/// 「产出物」跨会话索引:遍历 `~/.pinvou3/sessions/*.json`,把每个会话跟踪的
/// artifacts 汇成一张扁平表(供「产出物」一级入口用)。只走磁盘真相:
/// 文件已被删则跳过;mtime/size 现取 fs。
#[derive(Debug, Deserialize)]
struct DvSessionView {
    metadata: DvMeta,
    #[serde(default)]
    artifacts: Vec<DvArtifact>,
}
#[derive(Debug, Deserialize)]
struct DvMeta {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
}
#[derive(Debug, Deserialize)]
struct DvArtifact {
    storage_path: std::path::PathBuf,
    #[serde(default)]
    byte_size: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeliverableItem {
    name: String,
    path: String,
    ext: String,
    category: String,
    #[serde(rename = "sessionId")]
    session_id: String,
    source: String,
    mtime: i64,
    size: u64,
}

const DELIVERABLE_EXTS: &[&str] = &[
    "pptx", "ppt", "docx", "doc", "pdf", "html", "htm", "xlsx", "xls", "md", "csv", "png", "jpg",
    "jpeg", "svg", "gif", "webp", "zip",
];

fn deliverable_category(ext: &str) -> &'static str {
    match ext {
        "html" | "htm" | "mhtml" | "mht" => "web",
        "ppt" | "pptx" | "odp" | "dps" => "ppt",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "heic" => "img",
        _ => "doc",
    }
}

/// 「产出物」跨会话索引的装配实现(`list_deliverable_index` 命令薄壳调它)。
/// 只读磁盘真相:文件已删则跳过;mtime/size 现取 fs;同一物理路径只留最新。
pub(crate) fn list_deliverable_index_impl() -> Vec<DeliverableItem> {
    let sessions_dir = crate::platform::paths::sessions_root();
    let entries = match std::fs::read_dir(&sessions_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut by_path: std::collections::HashMap<String, DeliverableItem> =
        std::collections::HashMap::new();

    for entry in entries.flatten() {
        let file = entry.path();
        if !file.is_file() || file.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Ok(view) = serde_json::from_str::<DvSessionView>(&raw) else {
            continue;
        };

        for art in view.artifacts {
            let p = &art.storage_path;
            let Ok(meta) = std::fs::metadata(p) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let name = p
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                continue;
            }
            let ext = p
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            if !DELIVERABLE_EXTS.contains(&ext.as_str()) {
                continue;
            }
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let path = p.to_string_lossy().to_string();
            let item = DeliverableItem {
                name,
                path: path.clone(),
                ext: ext.clone(),
                category: deliverable_category(&ext).to_string(),
                session_id: view.metadata.id.clone(),
                source: view.metadata.title.clone(),
                mtime,
                size: if meta.len() > 0 {
                    meta.len()
                } else {
                    art.byte_size
                },
            };
            by_path
                .entry(path)
                .and_modify(|cur| {
                    if item.mtime >= cur.mtime {
                        *cur = item.clone();
                    }
                })
                .or_insert(item);
        }
    }

    let mut out: Vec<DeliverableItem> = by_path.into_values().collect();
    out.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| a.name.cmp(&b.name)));
    out
}

/// 奏折「成品箱」装配实现(`list_deliverables` 命令薄壳调它)。路径校验 +
/// canonicalize 留在命令层;这里接已校验的 `project_dir`(用于 join)与
/// `canon_root`(用于 starts_with 包含判断),完成全部装配。
///
/// 宝箱内容 = 奏折申报的成品(白浪定的原则,后端不猜):
/// - products:回奏官在 final_report.md「## 成品清单」段申报的文件(硬闸已核验
///   存在)。衙门式文件名(如 libu_1.md)物化成**以题目命名**的副本给客户——
///   题目取太子立项 zhiyi.json 的 title;非 md 成品(.pptx 等,名字本来就达意)
///   原样装箱。旧 run 没有成品清单段 → 回退:题目命名的 final_report 副本 +
///   deliverables/ 非 md 二进制成品。
/// - papers:deliverables/ 下未被申报为成品的 md = 六部过程文书,折叠降级。
pub(crate) fn assemble_deliverables(
    project_dir: &std::path::Path,
    canon_root: &std::path::Path,
) -> serde_json::Value {
    let title = std::fs::read_to_string(project_dir.join("_state").join("zhiyi.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("title").and_then(|t| t.as_str()).map(str::to_string));
    let title_base = sanitize_title_filename(title.as_deref().unwrap_or(""), "最终成品");

    let report_text =
        std::fs::read_to_string(project_dir.join("final_report.md")).unwrap_or_default();
    let declared = parse_product_manifest(&report_text);

    let mut products: Vec<serde_json::Value> = Vec::new();
    let mut product_canon: Vec<std::path::PathBuf> = Vec::new();

    if !declared.is_empty() {
        // 奏折申报路线:逐件解析,衙门式 md 文件名 → 题目命名副本
        let mut md_idx = 0usize;
        for rel in &declared {
            let cand = project_dir.join(rel);
            let Ok(canon) = std::fs::canonicalize(&cand) else {
                continue;
            };
            if !canon.starts_with(canon_root) || !canon.is_file() {
                continue;
            }
            let orig_name = canon
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let is_md = orig_name.to_lowercase().ends_with(".md");
            let bytes = std::fs::read(&canon).unwrap_or_default();
            if is_md {
                md_idx += 1;
                let fname = if md_idx == 1 {
                    format!("{title_base}.md")
                } else {
                    format!("{title_base}·{md_idx}.md")
                };
                let dst = project_dir.join(&fname);
                let stale = std::fs::read(&dst).map(|b| b != bytes).unwrap_or(true);
                if stale {
                    let _ = std::fs::write(&dst, &bytes);
                }
                let title = md_display_title(&bytes)
                    .unwrap_or_else(|| fname.trim_end_matches(".md").to_string());
                products.push(serde_json::json!({
                    "name": fname, "title": title, "path": dst.to_string_lossy(), "size": bytes.len(),
                }));
            } else {
                let stem = orig_name
                    .rsplit_once('.')
                    .map(|(a, _)| a)
                    .unwrap_or(&orig_name);
                products.push(serde_json::json!({
                    "name": orig_name, "title": stem, "path": canon.to_string_lossy(), "size": bytes.len(),
                }));
            }
            product_canon.push(canon);
        }
    }
    if products.is_empty() && !report_text.is_empty() {
        // 回退1(旧奏折没写成品清单):语义推断成品归属部——得分≥8(多路信号
        // 汇聚)才可信;取该部序号最大的 deliverable(整合终稿通常是末批)。
        let inferred = infer_product_bu(&report_text)
            .filter(|(_, score)| *score >= 8)
            .and_then(|(bu, _)| {
                let mut best: Option<(u32, std::path::PathBuf)> = None;
                if let Ok(entries) = std::fs::read_dir(project_dir.join("deliverables")) {
                    for e in entries.flatten() {
                        let path = e.path();
                        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                        if let Some(seq) = name
                            .strip_prefix(&format!("{bu}_"))
                            .and_then(|r| r.strip_suffix(".md"))
                            .and_then(|n| n.parse::<u32>().ok())
                        {
                            if best.as_ref().is_none_or(|(s, _)| seq > *s) {
                                best = Some((seq, path));
                            }
                        }
                    }
                }
                best.map(|(_, path)| path)
            });
        if let Some(src) = inferred {
            let bytes = std::fs::read(&src).unwrap_or_default();
            let fname = format!("{title_base}.md");
            let dst = project_dir.join(&fname);
            let stale = std::fs::read(&dst).map(|b| b != bytes).unwrap_or(true);
            if stale {
                let _ = std::fs::write(&dst, &bytes);
            }
            let title = md_display_title(&bytes)
                .unwrap_or_else(|| fname.trim_end_matches(".md").to_string());
            products.push(serde_json::json!({
                "name": fname, "title": title, "path": dst.to_string_lossy(), "size": bytes.len(),
            }));
            if let Ok(canon) = std::fs::canonicalize(&src) {
                product_canon.push(canon);
            }
        }
    }
    if products.is_empty() && !report_text.is_empty() {
        // 回退2(推断也不可信):题目命名的 final_report 副本,不至于空箱
        let fname = format!("{title_base}.md");
        let dst = project_dir.join(&fname);
        let stale = std::fs::read(&dst)
            .map(|b| b != report_text.as_bytes())
            .unwrap_or(true);
        if stale {
            let _ = std::fs::write(&dst, report_text.as_bytes());
        }
        products.push(serde_json::json!({
            "name": fname, "title": title_base.clone(), "path": dst.to_string_lossy(), "size": report_text.len(),
        }));
    }

    // deliverables/:非 md 且未申报 → 也算成品(工件清单核验过的二进制);
    // md 且未申报 → 过程文书
    let mut papers: Vec<serde_json::Value> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(project_dir.join("deliverables")) {
        let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            if !path.is_file() {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty()
                || name.starts_with('.')
                || name.ends_with(".tmp")
                || name.ends_with('~')
            {
                continue;
            }
            let canon = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if product_canon.contains(&canon) {
                continue; // 已申报装箱,不重复列
            }
            let size = path.metadata().map(|m| m.len()).unwrap_or(0);
            let is_md = name.to_lowercase().ends_with(".md");
            let title = if is_md {
                std::fs::read(&path).ok().and_then(|b| md_display_title(&b))
            } else {
                None
            }
            .unwrap_or_else(|| {
                name.rsplit_once('.')
                    .map(|(a, _)| a)
                    .unwrap_or(&name)
                    .to_string()
            });
            let item = serde_json::json!({
                "name": name, "title": title, "path": path.to_string_lossy(), "size": size,
            });
            if is_md {
                papers.push(item);
            } else {
                products.push(item);
            }
        }
    }
    serde_json::json!({ "products": products, "papers": papers })
}
