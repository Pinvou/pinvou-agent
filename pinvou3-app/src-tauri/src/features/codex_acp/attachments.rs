use std::fs;
use std::path::Path;

use agent_client_protocol::schema::v1::{
    ContentBlock, EmbeddedResource, EmbeddedResourceResource, ImageContent, PromptCapabilities,
    ResourceLink, TextContent, TextResourceContents,
};
use anyhow::{bail, Context, Result};
use base64::Engine;
use serde::Serialize;

use super::workspace::WorkspacePromptReference;
use crate::features::assistant::image_capability::EffectiveImageCapability;
use crate::features::files::file_ingest::{self, IngestResult};

const EMBED_FILE_MAX_TOKENS: u32 = 8_000;
const EMBED_TOTAL_MAX_TOKENS: u32 = 16_000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CodexDisplayAttachment {
    pub name: String,
    pub kind: String,
    pub size: u64,
}

pub(super) struct PreparedCodexPrompt {
    pub blocks: Vec<ContentBlock>,
    pub display_attachments: Vec<CodexDisplayAttachment>,
}

/// 组装 Codex ACP prompt。图片走 ACP 原生 `ContentBlock::Image`,发送前做设计
/// §6.4 的双层 gating:adapter 声明的 `capabilities.image` 只说明 Pinvou 可以把
/// 图片交给 ACP agent,底层当前模型能否识图由 `model_capability`
/// (`image_capability::acp_model_image_capability`)判定——文本模型与能力未知的
/// 模型都不能静默发送。
pub(super) fn prepare_codex_prompt(
    message: &str,
    attachments: &[IngestResult],
    workspace_references: &[WorkspacePromptReference],
    capabilities: &PromptCapabilities,
    model_capability: EffectiveImageCapability,
) -> Result<PreparedCodexPrompt> {
    let mut blocks = Vec::with_capacity(attachments.len() + workspace_references.len() + 1);
    if !message.trim().is_empty() {
        blocks.push(ContentBlock::Text(TextContent::new(message)));
    }

    let mut embedded_tokens = 0_u32;
    let mut display_attachments = Vec::with_capacity(attachments.len());
    for attachment in attachments {
        let path = file_ingest::validate_path(&attachment.path)
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("读取附件 {} 失败", attachment.basename))?;
        let uri = tauri::Url::from_file_path(&path)
            .map_err(|_| anyhow::anyhow!("无法构造附件文件地址: {}", path.display()))?
            .to_string();
        display_attachments.push(CodexDisplayAttachment {
            name: attachment.basename.clone(),
            kind: attachment.kind.clone(),
            size: attachment.byte_size,
        });

        if attachment.kind == "image" {
            if !capabilities.image {
                bail!(
                    "当前 Codex ACP Agent 未声明图片输入能力: {}",
                    attachment.basename
                );
            }
            match model_capability {
                EffectiveImageCapability::Unsupported => bail!(
                    "当前模型不支持图片输入，请切换到支持图片的模型: {}",
                    attachment.basename
                ),
                EffectiveImageCapability::Unknown => bail!(
                    "当前模型的图片输入能力未知，请在模型设置中将图片输入能力设为“支持图片”后重试，或切换到支持图片的模型: {}",
                    attachment.basename
                ),
                EffectiveImageCapability::Supported => {}
            }
            let data =
                fs::read(&path).with_context(|| format!("读取图片附件失败: {}", path.display()))?;
            blocks.push(ContentBlock::Image(
                ImageContent::new(
                    base64::engine::general_purpose::STANDARD.encode(data),
                    image_mime_type(&path)?,
                )
                .uri(uri),
            ));
            continue;
        }

        let can_embed = capabilities.embedded_context
            && attachment.markdown.is_some()
            && attachment.token_estimate <= EMBED_FILE_MAX_TOKENS
            && embedded_tokens.saturating_add(attachment.token_estimate) <= EMBED_TOTAL_MAX_TOKENS;
        if can_embed {
            embedded_tokens = embedded_tokens.saturating_add(attachment.token_estimate);
            let resource =
                TextResourceContents::new(attachment.markdown.as_deref().unwrap_or_default(), uri)
                    .mime_type(text_mime_type(&path));
            blocks.push(ContentBlock::Resource(EmbeddedResource::new(
                EmbeddedResourceResource::TextResourceContents(resource),
            )));
        } else {
            let size = i64::try_from(attachment.byte_size).unwrap_or(i64::MAX);
            let resource = ResourceLink::new(&attachment.basename, uri)
                .title(&attachment.basename)
                .size(size)
                .mime_type(resource_mime_type(&path, &attachment.kind));
            blocks.push(ContentBlock::ResourceLink(resource));
        }
    }

    for reference in workspace_references {
        let uri = tauri::Url::from_file_path(&reference.absolute_path)
            .map_err(|_| {
                anyhow::anyhow!(
                    "无法构造工作区文件地址: {}",
                    reference.absolute_path.display()
                )
            })?
            .to_string();
        let name = reference
            .absolute_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or(&reference.relative_path);
        let size = i64::try_from(reference.size).unwrap_or(i64::MAX);
        blocks.push(ContentBlock::ResourceLink(
            ResourceLink::new(&reference.relative_path, uri)
                .title(&reference.relative_path)
                .size(size)
                .mime_type(resource_mime_type(&reference.absolute_path, "workspace")),
        ));
        display_attachments.push(CodexDisplayAttachment {
            name: name.to_string(),
            kind: "workspace".to_string(),
            size: reference.size,
        });
    }

    if blocks.is_empty() {
        bail!("消息和附件不能同时为空");
    }
    Ok(PreparedCodexPrompt {
        blocks,
        display_attachments,
    })
}

fn image_mime_type(path: &Path) -> Result<&'static str> {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => Ok("image/png"),
        "jpg" | "jpeg" => Ok("image/jpeg"),
        "webp" => Ok("image/webp"),
        "gif" => Ok("image/gif"),
        other => bail!("Codex 不支持该图片格式: .{other}"),
    }
}

fn text_mime_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "md" | "markdown" => "text/markdown",
        "json" => "application/json",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        _ => "text/plain",
    }
}

fn resource_mime_type(path: &Path, kind: &str) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "pdf" => "application/pdf",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "zip" => "application/zip",
        _ if kind == "text" => text_mime_type(path),
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::current_dir()
                .unwrap()
                .join("target")
                .join(format!(
                    "pinvou3-codex-attachment-{label}-{}-{nonce}",
                    std::process::id()
                ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn attachment(path: &Path, kind: &str, markdown: Option<&str>, tokens: u32) -> IngestResult {
        IngestResult {
            kind: kind.to_string(),
            basename: path.file_name().unwrap().to_string_lossy().into_owned(),
            path: path.to_string_lossy().into_owned(),
            markdown: markdown.map(str::to_string),
            token_estimate: tokens,
            byte_size: fs::metadata(path).unwrap().len(),
            warning: None,
        }
    }

    #[test]
    fn image_uses_native_acp_content_block() {
        let dir = TestDir::new("image");
        let path = dir.path().join("image.png");
        fs::write(&path, b"png").unwrap();
        let capabilities = PromptCapabilities::new().image(true);
        let prepared = prepare_codex_prompt(
            "看图",
            &[attachment(&path, "image", None, 0)],
            &[],
            &capabilities,
            EffectiveImageCapability::Supported,
        )
        .unwrap();
        assert!(matches!(prepared.blocks[0], ContentBlock::Text(_)));
        assert!(matches!(prepared.blocks[1], ContentBlock::Image(_)));
        assert_eq!(prepared.display_attachments[0].name, "image.png");
    }

    #[test]
    fn small_text_uses_embedded_resource_and_large_text_uses_link() {
        let dir = TestDir::new("text");
        let small = dir.path().join("small.md");
        let large = dir.path().join("large.md");
        fs::File::create(&small)
            .unwrap()
            .write_all(b"small")
            .unwrap();
        fs::File::create(&large)
            .unwrap()
            .write_all(b"large")
            .unwrap();
        let capabilities = PromptCapabilities::new().embedded_context(true);
        // 文本附件不受模型图片能力影响:Unknown 模型照常 embed/link。
        let prepared = prepare_codex_prompt(
            "",
            &[
                attachment(&small, "text", Some("# small"), 10),
                attachment(&large, "text", Some("# large"), 8_001),
            ],
            &[],
            &capabilities,
            EffectiveImageCapability::Unknown,
        )
        .unwrap();
        assert!(matches!(prepared.blocks[0], ContentBlock::Resource(_)));
        assert!(matches!(prepared.blocks[1], ContentBlock::ResourceLink(_)));
    }

    #[test]
    fn image_requires_advertised_agent_capability() {
        let dir = TestDir::new("capability");
        let path = dir.path().join("image.png");
        fs::write(&path, b"png").unwrap();
        // adapter 不支持图片:无论模型能力如何都维持原拒绝文案。
        let error = prepare_codex_prompt(
            "",
            &[attachment(&path, "image", None, 0)],
            &[],
            &PromptCapabilities::default(),
            EffectiveImageCapability::Supported,
        )
        .err()
        .expect("image capability must be enforced");
        assert!(error.to_string().contains("未声明图片输入能力"));
    }

    #[test]
    fn image_rejected_when_model_is_text_only() {
        let dir = TestDir::new("text-model");
        let path = dir.path().join("image.png");
        fs::write(&path, b"png").unwrap();
        let capabilities = PromptCapabilities::new().image(true);
        let error = prepare_codex_prompt(
            "",
            &[attachment(&path, "image", None, 0)],
            &[],
            &capabilities,
            EffectiveImageCapability::Unsupported,
        )
        .err()
        .expect("text-only model must not receive images");
        assert!(error.to_string().contains("当前模型不支持图片输入"));
    }

    #[test]
    fn image_rejected_when_model_capability_unknown() {
        let dir = TestDir::new("unknown-model");
        let path = dir.path().join("image.png");
        fs::write(&path, b"png").unwrap();
        let capabilities = PromptCapabilities::new().image(true);
        let error = prepare_codex_prompt(
            "",
            &[attachment(&path, "image", None, 0)],
            &[],
            &capabilities,
            EffectiveImageCapability::Unknown,
        )
        .err()
        .expect("unknown model capability must not silently send images");
        let message = error.to_string();
        assert!(message.contains("图片输入能力未知"));
        // 提示必须给出 override Enabled 这一用户显式确认出口。
        assert!(message.contains("支持图片"));
    }
}
