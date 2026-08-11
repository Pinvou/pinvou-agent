/// 把图片附件拷进 session workspace 的 `attachments/` 子目录,返回供 `image_analyze`
/// 使用的 **workspace 相对路径**(image_analyze 只接受不逃逸 workspace 的相对路径)。
/// 失败返回 None,上层降级为提示无法读图。
pub(super) fn validate_staged_attachment_basename(basename: &str) -> Result<(), String> {
    if basename.is_empty() {
        return Err("basename is empty".to_string());
    }
    if basename.contains('/') || basename.contains('\\') {
        return Err("basename must not contain path separators".to_string());
    }
    let bytes = basename.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err("basename must not contain a drive prefix".to_string());
    }

    let mut components = std::path::Path::new(basename).components();
    match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(_)), None) => Ok(()),
        _ => Err("basename must be exactly one normal path component".to_string()),
    }
}

// Staging security boundary:
// - basename validation blocks traversal, absolute paths, drive prefixes, and separators;
// - canonical parent walking rejects pre-existing symlink/junction escapes;
// - create_new prevents overwriting an existing file or following a pre-existing target link;
// - exclusive reservation gives benign concurrent writers distinct names.
//
// This does not claim to defeat a malicious process that already has write access to the same
// workspace and actively swaps a parent between validation and open. Such a process already has
// equivalent write authority; closing that residual race requires platform-specific handle-relative
// APIs (for example openat-style directory handles), deliberately outside this local staging helper.
fn prepare_staging_directory(
    workspace: &std::path::Path,
    attachment_dir: &str,
) -> Option<(String, std::path::PathBuf, std::path::PathBuf)> {
    let attachment_dir = attachment_dir.trim_end_matches('/');
    let relative = std::path::Path::new(attachment_dir);
    if relative.as_os_str().is_empty() || relative.is_absolute() {
        return None;
    }
    let components = relative
        .components()
        .map(|component| match component {
            std::path::Component::Normal(name) => Some(name.to_os_string()),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;

    std::fs::create_dir_all(workspace).ok()?;
    let canonical_workspace = std::fs::canonicalize(workspace).ok()?;
    let mut canonical_parent = canonical_workspace.clone();
    for component in components {
        canonical_parent.push(component);
        match std::fs::create_dir(&canonical_parent) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return None,
        }
        let metadata = std::fs::symlink_metadata(&canonical_parent).ok()?;
        if !metadata.is_dir() {
            return None;
        }
        let resolved = std::fs::canonicalize(&canonical_parent).ok()?;
        if !resolved.starts_with(&canonical_workspace) {
            return None;
        }
        // Continue from the resolved parent rather than the user-visible path.
        // A pre-existing link can therefore never redirect creation of the
        // next component outside the canonical execution workspace.
        canonical_parent = resolved;
    }

    Some((
        attachment_dir.to_string(),
        canonical_workspace,
        canonical_parent,
    ))
}

fn reserve_unique_staged_file(
    directory: &std::path::Path,
    initial_name: String,
    stem: &str,
    suffix: &str,
) -> Option<(std::fs::File, std::path::PathBuf, String)> {
    const MAX_CANDIDATES: usize = 10_000;
    for attempt in 0..MAX_CANDIDATES {
        let candidate = if attempt == 0 {
            initial_name.clone()
        } else {
            format!("{stem}-{attempt}{suffix}")
        };
        let path = directory.join(&candidate);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Some((file, path, candidate)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

fn staged_reserved_target_is_unchanged(file: &std::fs::File, path: &std::path::Path) -> bool {
    crate::platform::filesystem::reserved_target_is_unchanged(file, path)
}

fn staged_target_is_safe(
    file: &std::fs::File,
    path: &std::path::Path,
    canonical_workspace: &std::path::Path,
) -> bool {
    staged_reserved_target_is_unchanged(file, path)
        && std::fs::canonicalize(path)
            .is_ok_and(|resolved| resolved.starts_with(canonical_workspace))
}

pub(super) fn stage_image_in_workspace(
    src: &str,
    basename: &str,
    workspace: &std::path::Path,
    attachment_dir: &str,
) -> Option<String> {
    validate_staged_attachment_basename(basename).ok()?;
    let (attachment_dir, canonical_workspace, directory) =
        prepare_staging_directory(workspace, attachment_dir)?;
    let mut source = std::fs::File::open(src).ok()?;
    let (stem, suffix) = match basename.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (basename.to_string(), String::new()),
    };
    let (mut destination, path, candidate) =
        reserve_unique_staged_file(&directory, basename.to_string(), &stem, &suffix)?;
    if !staged_target_is_safe(&destination, &path, &canonical_workspace) {
        return None;
    }
    if std::io::copy(&mut source, &mut destination).is_err() {
        return None;
    }
    if !staged_target_is_safe(&destination, &path, &canonical_workspace) {
        return None;
    }
    Some(format!("{attachment_dir}/{candidate}"))
}

/// Reuse a session-owned attachment that is already inside the execution
/// workspace. HTML5 desktop drops are committed there directly, so copying an
/// image again would create an unnecessary second application-owned copy.
pub(super) fn existing_workspace_relative_file(
    src: &str,
    workspace: &std::path::Path,
) -> Option<String> {
    let canonical_workspace = std::fs::canonicalize(workspace).ok()?;
    let canonical_source = std::fs::canonicalize(src).ok()?;
    if !canonical_source.is_file() || !canonical_source.starts_with(&canonical_workspace) {
        return None;
    }
    let relative = canonical_source.strip_prefix(canonical_workspace).ok()?;
    let parts = relative
        .components()
        .map(|component| match component {
            std::path::Component::Normal(value) => value.to_str().filter(|value| !value.is_empty()),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

/// Copy a remote-control upload into the session workspace before the
/// temporary upload directory is removed.
#[allow(dead_code)]
pub(crate) fn stage_remote_attachment_source(
    src: &str,
    basename: &str,
    workspace: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let relative =
        stage_image_in_workspace(src, basename, workspace, ".pinvou3/remote-attachments")?;
    Some(workspace.join(relative))
}

/// 附件内联预算(token 估算)。单文件超过 INLINE_MAX、或多附件累计超过 TOTAL_BUDGET
/// 的部分,不再全量嵌入 prompt——256K 窗口一条消息就能撑爆(实测 5000 行 xlsx 转
/// CSV ≈ 237K tokens,直接顶穿 vLLM 262144 上限),且即使不炸窗口,小模型在超长
/// 内联里的注意力质量也差。超限附件改注入「落盘路径 + 预览」,引导模型按需
/// read_file 分页 / exec_shell 聚合(底座 read_file 原生支持 start_line/max_lines)。
pub(super) const ATTACH_INLINE_MAX_TOKENS: u32 = 8_000;
const ATTACH_TOTAL_BUDGET_TOKENS: u32 = 16_000;
/// 路径模式的开头预览:行数与字符双上限,先到为准。
const ATTACH_PREVIEW_LINES: usize = 20;
const ATTACH_PREVIEW_MAX_CHARS: usize = 1_500;

/// 把超限附件的转换产物写进指定的 workspace 相对目录(防重名递增),返回
/// workspace 相对路径。普通对话的 text 仍直接使用原路径；scheduled 对话会复制
/// 到 run 专属目录，避免无人值守引擎依赖 workspace 外路径。
pub(super) fn stage_text_in_workspace(
    content: &str,
    basename: &str,
    ext: &str,
    workspace: &std::path::Path,
    attachment_dir: &str,
) -> Option<String> {
    use std::io::Write as _;
    stage_text_in_workspace_with_writer(
        basename,
        ext,
        workspace,
        attachment_dir,
        |destination, _path| destination.write_all(content.as_bytes()),
    )
}

pub(super) fn stage_text_in_workspace_with_writer<F>(
    basename: &str,
    ext: &str,
    workspace: &std::path::Path,
    attachment_dir: &str,
    writer: F,
) -> Option<String>
where
    F: FnOnce(&mut std::fs::File, &std::path::Path) -> std::io::Result<()>,
{
    validate_staged_attachment_basename(basename).ok()?;
    let (attachment_dir, canonical_workspace, directory) =
        prepare_staging_directory(workspace, attachment_dir)?;
    let stem = basename.rsplit_once('.').map_or(basename, |(s, _)| s);
    let suffix = format!(".{ext}");
    let (mut destination, path, candidate) =
        reserve_unique_staged_file(&directory, format!("{stem}{suffix}"), stem, &suffix)?;
    if !staged_target_is_safe(&destination, &path, &canonical_workspace) {
        return None;
    }
    if writer(&mut destination, &path).is_err() {
        // Deliberately leave the app-named orphan in place. Unlinking by path after a failed
        // post-check would introduce another check-then-unlink race and could delete a replacement
        // installed by a concurrent writer.
        return None;
    }
    if !staged_target_is_safe(&destination, &path, &canonical_workspace) {
        return None;
    }
    Some(format!("{attachment_dir}/{candidate}"))
}

/// 转换产物落盘时的扩展名:表格是 CSV(awk/python 可直接吃),pandoc 产物是
/// markdown,其余(pdftotext/LibreOffice txt/邮件)是纯文本。
fn converted_ext(kind: &str) -> &'static str {
    match kind {
        "xlsx" | "ods" | "xls" | "et" => "csv",
        "docx" | "odt" | "archive" => "md",
        _ => "txt",
    }
}

/// 取 markdown 开头若干行做预览,返回 (预览, 总行数)。
fn attachment_preview(md: &str) -> (String, usize) {
    let total_lines = md.lines().count();
    let mut preview = String::new();
    for (i, line) in md.lines().enumerate() {
        if i >= ATTACH_PREVIEW_LINES
            || preview.chars().count() + line.chars().count() > ATTACH_PREVIEW_MAX_CHARS
        {
            break;
        }
        preview.push_str(line);
        preview.push('\n');
    }
    (preview, total_lines)
}

/// 超限附件的注入段:落盘(text 类直接用原始路径)+ 预览 + 工具引导。
/// 显式声明「只看到预览」——否则小模型会拿前 20 行当全量数据静默作答。
/// `reference_absolute`：落盘根与引擎 cwd 不同根（原生代码会话绑项目目录）时
/// 引用绝对路径，避免引擎按项目目录解析私有目录的相对路径而落空。
fn push_large_attachment_section(
    out: &mut String,
    a: &crate::features::files::file_ingest::IngestResult,
    md: &str,
    workspace: &std::path::Path,
    attachment_dir: &str,
    stage_original_text: bool,
    reference_absolute: bool,
) {
    let read_path = if a.kind == "text" && !stage_original_text {
        a.path.clone()
    } else {
        match stage_text_in_workspace(
            md,
            &a.basename,
            converted_ext(&a.kind),
            workspace,
            attachment_dir,
        ) {
            Some(rel) => staged_reference(workspace, &rel, reference_absolute),
            None => {
                out.push_str(
                    "⚠️ 此文件过大无法内嵌,且转换产物落盘失败。请告知用户该附件无法处理,\
                     不要臆测其内容。\n",
                );
                return;
            }
        }
    };
    let (preview, total_lines) = attachment_preview(md);
    out.push_str(&format!(
        "⚠️ 此文件约 ~{} tokens,过大,完整内容**没有**嵌入本消息。你只看到下面的开头预览,\
         **绝不能**只凭预览回答涉及全文/全表的问题。\n\
         完整内容已是纯文本,共 {} 行,路径: `{}`\n\
         预览(仅开头几行):\n```\n{}```\n\
         需要完整内容时:\n\
         - 统计/筛选/聚合(尤其表格数据):优先用 exec_shell 写 awk 或 python 一次算出结果,不要逐页通读\n\
         - 通读/定位:用 read_file 分页(start_line/max_lines;返回 truncated=\"true\" 时按 next_start_line 续读)\n",
        a.token_estimate, total_lines, read_path, preview
    ));
}

/// 落盘附件在消息里的引用形式：默认 workspace 相对路径（引擎 cwd 即落盘根）；
/// 落盘根与引擎 cwd 不同根时（原生代码会话绑项目目录）引用绝对路径。
fn staged_reference(
    workspace: &std::path::Path,
    relative: &str,
    reference_absolute: bool,
) -> String {
    if reference_absolute {
        workspace.join(relative).to_string_lossy().into_owned()
    } else {
        relative.to_string()
    }
}

/// 按指定 workspace 相对目录拼接 user 文本 + 附件 markdown。
/// 图片拷进 workspace 后引导 LLM 调 image_analyze 读图(Qwen3.6 有视觉能力);
/// 文本类附件按 token 预算分流:小→全量内联,大→落盘+路径+预览(见常量注释)。
/// `reference_absolute` 见 `staged_reference`；普通对话与 scheduled 传 false。
pub(super) fn build_message_with_attachments_in_dir(
    text: String,
    attachments: Vec<crate::features::files::file_ingest::IngestResult>,
    workspace: &std::path::Path,
    attachment_dir: &str,
    reference_absolute: bool,
) -> String {
    if attachments.is_empty() {
        return text;
    }
    let mut out = String::new();
    if !text.trim().is_empty() {
        out.push_str(&text);
        out.push_str("\n\n");
    }
    out.push_str("---\n用户附上了以下文件:\n\n");
    let mut inline_spent: u32 = 0;
    for a in &attachments {
        out.push_str(&format!(
            "### {} ({}, {} bytes",
            a.basename, a.kind, a.byte_size
        ));
        if a.token_estimate > 0 {
            out.push_str(&format!(", ~{} tokens", a.token_estimate));
        }
        out.push_str(")\n");
        // 真实路径 —— AI 如果一定要 read_file 也能找到对的位置，
        // 同时避免 AI 凭想象编造 workspace/<timestamp>-... 这种伪路径
        out.push_str(&format!("原始路径: `{}`\n", a.path));
        if a.kind == "image" {
            // 把图拷进 workspace,硬约束引导 LLM 调 image_analyze 读图。
            // 关键:不能说"你有视觉能力"——那会让模型以为可直接描述而凭空幻觉
            // (实测同一张图,不调工具时编造内容,调工具才得真相)。改成"你现在
            // 一无所知,调用前绝不描述",把模糊建议变成具体硬规则(Qwen3.6 对具体
            // 硬规则遵循好、对抽象意图无效)。
            match existing_workspace_relative_file(&a.path, workspace).or_else(|| {
                stage_image_in_workspace(&a.path, &a.basename, workspace, attachment_dir)
            }) {
                Some(rel) => {
                    let rel = staged_reference(workspace, &rel, reference_absolute);
                    out.push_str(&format!(
                        "🖼 用户附了一张图片,存在 workspace 的 `{rel}`。\n\
                        ⚠️ 你现在**看不到这张图的任何内容**,对图里有什么**一无所知**。\
                        在调用 image_analyze 工具并拿到返回结果之前,你**绝对不能**描述、\
                        猜测或编造图里有什么——包括「这是什么」「帅吗」「什么颜色」「是不是某某文档」\
                        这类**任何**关于图的问题。凭空作答=幻觉,是严重错误。\n\
                        要回答**任何**跟这张图有关的问题,**必须先**调用:\n\
                        `image_analyze(image_path=\"{rel}\", prompt=\"<按用户问题要看的,如:描述这张图/读出文字/这是什么>\")`\n\
                        拿到工具返回的描述后,再据此如实回答用户。\n",
                    ));
                }
                None => {
                    out.push_str(
                        "⚠️ 这张图片暂存到 workspace 失败,无法用 image_analyze 读取。\
                        请告知用户图片无法处理,不要臆测图里的内容。\n",
                    );
                }
            }
        } else if let Some(md) = &a.markdown {
            let fits = a.token_estimate <= ATTACH_INLINE_MAX_TOKENS
                && inline_spent.saturating_add(a.token_estimate) <= ATTACH_TOTAL_BUDGET_TOKENS;
            if fits {
                inline_spent = inline_spent.saturating_add(a.token_estimate);
                out.push_str(
                    "**以下代码块是文件完整内容,可直接使用,不需要再调 read_file / \
                     file_search 重新读取。**如需保存修改版本,用 write_file 写到 \
                     PINVOU3_WORKSPACE 下;单个文件过大时拆分为多个有明确用途的文件。\n",
                );
                out.push_str("```\n");
                out.push_str(md);
                if !md.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str("```\n");
            } else {
                push_large_attachment_section(
                    &mut out,
                    a,
                    md,
                    workspace,
                    attachment_dir,
                    attachment_dir != "attachments",
                    reference_absolute,
                );
            }
        } else if let Some(warning) = &a.warning {
            out.push_str(&format!("⚠️ {warning}\n"));
        }
        out.push('\n');
    }
    out.push_str("---\n");
    out
}

/// 普通对话附件入口。pub 仅为 L1 dialog harness 复用(lib.rs re-export)，
/// 不是对外 API；scheduled chat 走上面的 run 专属目录入口。
pub fn build_message_with_attachments(
    text: String,
    attachments: Vec<crate::features::files::file_ingest::IngestResult>,
    workspace: &std::path::Path,
) -> String {
    build_message_with_attachments_in_dir(text, attachments, workspace, "attachments", false)
}

// ─────────────────────────────────────────────────────────────────────────────
// 原生图片输入(设计 §9.1/§9.2,阶段 D)
// ─────────────────────────────────────────────────────────────────────────────

/// UI/timeline 展示文本:附件名以 📎 拼接(图片/文件同式)。
/// chat.rs 的 Fallback/无图路径与 Native 路径共用,保持两处不漂移。
pub(super) fn attachment_display_text(
    text: &str,
    attachments: &[crate::features::files::file_ingest::IngestResult],
) -> String {
    if attachments.is_empty() {
        return text.to_string();
    }
    let names = attachments
        .iter()
        .map(|attachment| attachment.basename.as_str())
        .collect::<Vec<_>>()
        .join(" · ");
    if text.trim().is_empty() {
        format!("📎 {names}")
    } else {
        format!("{text}\n\n📎 {names}")
    }
}

/// Native 路径(v0.9.5 底座官方标记方案)消息构造:图片暂存到 workspace 后
/// 在消息文本中生成 `[Attached image: <path>]` 标记行,由底座
/// `image_attach::expand_attachment_blocks` 在构建时展开为
/// `ContentBlock::ImageUrl`(data URL),并按 route 能力在请求前剥离。
/// **不注入**"看不到图"硬规则、不引导 image_analyze;非图片附件沿用现有
/// token 预算分流文本段(与 build_message_with_attachments_in_dir 同规则)。
/// 标记使用暂存后的绝对路径(引擎以主进程 cwd 读文件,相对路径不可靠),
/// 不带用户原始路径(设计 §10.1)。任一图片暂存失败即 Err——不静默降级
/// 为纯文本(设计 §10.2)。
pub(super) fn prepare_native_user_message_in_dir(
    text: String,
    attachments: Vec<crate::features::files::file_ingest::IngestResult>,
    workspace: &std::path::Path,
    attachment_dir: &str,
) -> Result<String, String> {
    let mut inline_spent: u32 = 0;
    let mut segment = String::new();
    if !text.trim().is_empty() {
        segment.push_str(&text);
        if !attachments.is_empty() {
            segment.push_str("\n\n");
        }
    }
    if !attachments.is_empty() {
        segment.push_str("---\n用户附上了以下文件:\n\n");
    }
    for a in &attachments {
        if a.kind == "image" {
            // 暂存复用现有校验链(basename 白名单/symlink 防逃逸/create_new 防覆盖);
            // 标记路径只来自暂存结果,不接受前端直给路径(设计 §11)。
            let relative =
                stage_image_in_workspace(&a.path, &a.basename, workspace, attachment_dir)
                    .ok_or_else(|| {
                        format!(
                            "图片 {} 暂存到 workspace 失败,无法原生发送。请重新选择图片。",
                            a.basename
                        )
                    })?;
            let abs = workspace.join(&relative);
            segment.push_str(&format!(
                "### {} (image, {} bytes)\n",
                a.basename, a.byte_size
            ));
            // 官方标记:底座构建消息时按行解析并展开为 ImageUrl 块。
            segment.push_str(&format!("[Attached image: {}]\n\n", abs.display()));
            continue;
        }
        segment.push_str(&format!(
            "### {} ({}, {} bytes",
            a.basename, a.kind, a.byte_size
        ));
        if a.token_estimate > 0 {
            segment.push_str(&format!(", ~{} tokens", a.token_estimate));
        }
        segment.push_str(")\n");
        // 真实路径 —— AI 如果一定要 read_file 也能找到对的位置(与文本路径同行为)
        segment.push_str(&format!("原始路径: `{}`\n", a.path));
        if let Some(md) = &a.markdown {
            let fits = a.token_estimate <= ATTACH_INLINE_MAX_TOKENS
                && inline_spent.saturating_add(a.token_estimate) <= ATTACH_TOTAL_BUDGET_TOKENS;
            if fits {
                inline_spent = inline_spent.saturating_add(a.token_estimate);
                segment.push_str(
                    "**以下代码块是文件完整内容,可直接使用,不需要再调 read_file / \
                     file_search 重新读取。**如需保存修改版本,用 write_file 写到 \
                     PINVOU3_WORKSPACE 下;大产物用 append_file 分块追加。\n",
                );
                segment.push_str("```\n");
                segment.push_str(md);
                if !md.ends_with('\n') {
                    segment.push('\n');
                }
                segment.push_str("```\n");
            } else {
                push_large_attachment_section(
                    &mut segment,
                    a,
                    md,
                    workspace,
                    attachment_dir,
                    attachment_dir != "attachments",
                    // Native 分支图片/文件都暂存到执行根,落盘根与引擎 cwd 一致,相对引用。
                    false,
                );
            }
        } else if let Some(warning) = &a.warning {
            segment.push_str(&format!("⚠️ {warning}\n"));
        }
        segment.push('\n');
    }
    if !attachments.is_empty() {
        segment.push_str("---\n");
    }
    Ok(segment)
}
