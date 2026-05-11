//! AgentRegistry — 扫描 `prompts/*.md`，解析 YAML frontmatter，注册 agent。
//!
//! 取代旧的 `apps/<App名>/app.toml` 目录结构。
//!
//! 文件格式：
//! ```markdown
//! ---
//! id: doc_generation
//! name: 文档生成
//! description: 根据用户提供的素材生成结构化文档...
//! emoji: 📝
//! ---
//!
//! # 角色
//! ...
//! ```
//!
//! frontmatter 为简单的 `key: value` 行（不支持嵌套/列表/多行值），
//! 这样无需引入 serde_yaml 依赖。

use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// 单个 agent 的元数据 + 正文
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDefinition {
    /// 程序内唯一标识（与文件名一致）
    pub id: String,
    /// 显示名称
    pub name: String,
    /// 一句话场景描述，给 LLM 看用于选 agent
    pub description: String,
    /// UI 图标（可选）
    pub emoji: Option<String>,
    /// system prompt 正文（frontmatter 之后的内容）
    pub body: String,
}

/// Agent 注册表
#[derive(Debug, Clone, Default)]
pub struct AgentRegistry {
    agents: HashMap<String, AgentDefinition>,
    /// 保留扫描顺序（按字典序），用于稳定输出
    order: Vec<String>,
}

impl AgentRegistry {
    /// 扫描指定目录下所有 `.md` 文件，解析并注册。
    ///
    /// 跳过：
    /// - 解析失败的文件（记录但不 panic）
    /// - frontmatter 缺失 `id` / `name` / `description` 的文件
    /// - `id` 与文件名 stem 不一致的文件
    /// - `id` 重复的文件（后扫到的覆盖先扫到的）
    pub fn from_directory(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        if !dir.exists() {
            bail!("prompts dir not found: {}", dir.display());
        }
        if !dir.is_dir() {
            bail!("not a directory: {}", dir.display());
        }

        let mut entries: Vec<PathBuf> = fs::read_dir(dir)
            .with_context(|| format!("read_dir {}", dir.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md"))
            .collect();
        entries.sort();

        let mut registry = Self::default();
        for path in entries {
            match Self::load_one(&path) {
                Ok(agent) => registry.register(agent),
                Err(err) => {
                    eprintln!("[agent_registry] skip {}: {}", path.display(), err);
                }
            }
        }
        Ok(registry)
    }

    /// 解析单个 `.md` 文件
    pub fn load_one(path: &Path) -> Result<AgentDefinition> {
        let content =
            fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let (frontmatter, body) = split_frontmatter(&content)?;
        let fields = parse_frontmatter(frontmatter)?;

        let id = fields
            .get("id")
            .cloned()
            .context("frontmatter missing 'id'")?;
        let name = fields
            .get("name")
            .cloned()
            .context("frontmatter missing 'name'")?;
        let description = fields
            .get("description")
            .cloned()
            .context("frontmatter missing 'description'")?;
        let emoji = fields.get("emoji").cloned();

        // 校验 id 与文件名一致
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .context("invalid filename")?;
        if id != stem {
            bail!("frontmatter id '{}' does not match filename stem '{}'", id, stem);
        }

        Ok(AgentDefinition {
            id,
            name,
            description,
            emoji,
            body: body.trim().to_string(),
        })
    }

    pub fn register(&mut self, agent: AgentDefinition) {
        if !self.agents.contains_key(&agent.id) {
            self.order.push(agent.id.clone());
        }
        self.agents.insert(agent.id.clone(), agent);
    }

    pub fn get(&self, id: &str) -> Option<&AgentDefinition> {
        self.agents.get(id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.agents.contains_key(id)
    }

    pub fn len(&self) -> usize {
        self.agents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    /// 按注册顺序遍历所有 agent
    pub fn iter(&self) -> impl Iterator<Item = &AgentDefinition> {
        self.order.iter().filter_map(|id| self.agents.get(id))
    }

    /// 拼接成 CombinedPlanner prompt 中的 agent 列表
    ///
    /// 形如：
    /// ```text
    /// - qa: 简单问答、概念解释、翻译、闲聊。无需多步拆解
    /// - doc_generation: 根据用户素材生成结构化文档...
    /// ```
    pub fn render_for_planner(&self) -> String {
        self.iter()
            .map(|a| format!("- {}: {}", a.id, a.description))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

// --- frontmatter 解析 ---

fn split_frontmatter(content: &str) -> Result<(&str, &str)> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        bail!("missing opening '---' frontmatter delimiter");
    }
    // 跳过开头的 `---\n`
    let after_open = &trimmed[3..];
    let after_open = after_open.trim_start_matches('\r').trim_start_matches('\n');
    // 找下一个 `\n---` 边界
    let end = after_open
        .find("\n---")
        .context("missing closing '---' frontmatter delimiter")?;
    let frontmatter = &after_open[..end];
    let body_start = end + "\n---".len();
    let body = after_open[body_start..]
        .trim_start_matches('\r')
        .trim_start_matches('\n');
    Ok((frontmatter, body))
}

fn parse_frontmatter(text: &str) -> Result<HashMap<String, String>> {
    let mut fields = HashMap::new();
    for (idx, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once(':')
            .with_context(|| format!("invalid frontmatter line {}: '{}'", idx + 1, line))?;
        let key = key.trim().to_string();
        if key.is_empty() {
            bail!("empty key at frontmatter line {}", idx + 1);
        }
        // 去除 value 两侧的引号（如果有）
        let value = value.trim();
        let value = value
            .trim_start_matches('"')
            .trim_end_matches('"')
            .trim_start_matches('\'')
            .trim_end_matches('\'')
            .to_string();
        fields.insert(key, value);
    }
    Ok(fields)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn write_md(dir: &Path, name: &str, content: &str) {
        let path = dir.join(name);
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn parse_basic_frontmatter() {
        let content = "---\nid: qa\nname: 简单问答\ndescription: 描述\nemoji: 💬\n---\n\n# 角色\n你是助手";
        let (fm, body) = split_frontmatter(content).unwrap();
        let fields = parse_frontmatter(fm).unwrap();
        assert_eq!(fields.get("id").unwrap(), "qa");
        assert_eq!(fields.get("name").unwrap(), "简单问答");
        assert_eq!(fields.get("description").unwrap(), "描述");
        assert_eq!(fields.get("emoji").unwrap(), "💬");
        assert!(body.starts_with("# 角色"));
    }

    #[test]
    fn parse_frontmatter_handles_quoted_values() {
        let content = "---\nid: \"qa\"\nname: '简单问答'\ndescription: 一句话\n---\nbody";
        let (fm, _) = split_frontmatter(content).unwrap();
        let fields = parse_frontmatter(fm).unwrap();
        assert_eq!(fields.get("id").unwrap(), "qa");
        assert_eq!(fields.get("name").unwrap(), "简单问答");
    }

    #[test]
    fn missing_opening_delimiter_errors() {
        let content = "id: qa\nname: x\n---\nbody";
        assert!(split_frontmatter(content).is_err());
    }

    #[test]
    fn missing_closing_delimiter_errors() {
        let content = "---\nid: qa\nname: x\nbody";
        assert!(split_frontmatter(content).is_err());
    }

    #[test]
    fn loads_directory_with_multiple_files() {
        let tmp = TempDir::new().unwrap();
        write_md(
            tmp.path(),
            "qa.md",
            "---\nid: qa\nname: 简单问答\ndescription: Q&A\n---\nbody-qa",
        );
        write_md(
            tmp.path(),
            "doc_generation.md",
            "---\nid: doc_generation\nname: 文档生成\ndescription: Docs\nemoji: 📝\n---\nbody-doc",
        );

        let reg = AgentRegistry::from_directory(tmp.path()).unwrap();
        assert_eq!(reg.len(), 2);
        assert!(reg.contains("qa"));
        assert!(reg.contains("doc_generation"));

        let doc = reg.get("doc_generation").unwrap();
        assert_eq!(doc.name, "文档生成");
        assert_eq!(doc.emoji.as_deref(), Some("📝"));
        assert_eq!(doc.body, "body-doc");
    }

    #[test]
    fn skips_files_with_id_mismatch() {
        let tmp = TempDir::new().unwrap();
        // 文件名 wrong_name.md，但 id 写成 qa
        write_md(
            tmp.path(),
            "wrong_name.md",
            "---\nid: qa\nname: X\ndescription: x\n---\nbody",
        );
        let reg = AgentRegistry::from_directory(tmp.path()).unwrap();
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn skips_files_missing_required_field() {
        let tmp = TempDir::new().unwrap();
        write_md(
            tmp.path(),
            "qa.md",
            "---\nid: qa\nname: X\n---\nbody", // 缺 description
        );
        let reg = AgentRegistry::from_directory(tmp.path()).unwrap();
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn render_for_planner_lists_all_agents() {
        let tmp = TempDir::new().unwrap();
        write_md(
            tmp.path(),
            "qa.md",
            "---\nid: qa\nname: X\ndescription: Q&A\n---\nbody",
        );
        write_md(
            tmp.path(),
            "planning.md",
            "---\nid: planning\nname: P\ndescription: Plans\n---\nbody",
        );
        let reg = AgentRegistry::from_directory(tmp.path()).unwrap();
        let rendered = reg.render_for_planner();
        assert!(rendered.contains("- qa: Q&A"));
        assert!(rendered.contains("- planning: Plans"));
    }

    #[test]
    fn ignores_non_md_files() {
        let tmp = TempDir::new().unwrap();
        write_md(
            tmp.path(),
            "qa.md",
            "---\nid: qa\nname: X\ndescription: x\n---\nbody",
        );
        write_md(tmp.path(), "README.txt", "not an agent");
        let reg = AgentRegistry::from_directory(tmp.path()).unwrap();
        assert_eq!(reg.len(), 1);
    }
}
