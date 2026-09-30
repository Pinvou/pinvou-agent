//! 会话 fork：把源会话的线性前缀复制为一个全新会话，源会话原封不动
//!（`docs/fork-session-plan.md` §4）。
//!
//! 复制口径（§4.1）：
//! - `messages`：`load_session_snapshot` 的活跃线性投影按 [`deepseek_tui::is_user_turn_prompt`]
//!   切出的用户 turn 前缀（与 rewind / checkpoints 同谓词，D3）；journal 由
//!   `create_saved_session_with_id_and_mode` 从前缀重建——单根单链，死分支/多根不随行（D2）。
//! - `system_prompt`（压缩摘要随行继承）、`context_references`、artifacts 元数据
//!   + 会话私有目录里的 artifact / ledger 内容（D9）、模型（含 provider）与
//!   `_session_models.json`、`_session_mode_states.json`。
//! - 不复制：置顶 / 收起、steered / turn timeline / 血缘元数据（D1：标题后缀可见性更好）、
//!   cost 快照（新会话独立计费）、multi-agent 开关。
//!
//! 工作区（§2.3）：钥匙串 = 主根 + 附加根（PR484 sidecar）。按 [`ForkWorkspacePlan`]
//! 逐根隔离（git 工作树 / 目录复制，自动选择不暴露给用户，D5），共享根保留原路径；
//! 隔离根的路径映射以普通用户消息形态注入新会话开头（D10，不改写历史路径）。
//!
//! 失败语义（§6.1 #10）：任一步失败整体回滚——不留半成品会话记录，不留半成品
//! 副本目录 / 工作树，源会话与源磁盘状态不受影响。
//!
//! 守卫窗口：`scheduled_mutation` 只覆盖「读快照→切前缀」与「落新记录」两个
//! 短窗口；隔离复制是慢 IO，故意不持锁（否则一次多 GB 复制会冻结所有会话的
//! 持久化）。窗口间源会话完成新 turn 至多让 fork 拿到 turn 前的旧快照——仍是一致
//! 前缀（turn 持久化本身在锁内原子完成）；「生成中拒绝」由命令层 + 前端置灰
//! 双保险（D12），与本层正交。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use deepseek_tui::artifacts::ArtifactRecord;
use deepseek_tui::models::{ContentBlock, Message};
use deepseek_tui::session_manager::create_saved_session_with_id_and_mode;

use super::SessionStore;
use super::store::NEW_CHAT_TITLE;
use super::validators::{generate_session_id, validate_session_id};
use crate::platform::workspace_isolation::{
    self as isolation, CopyProgress, GitProgram, IsolatedRoot,
};

/// 标题后缀（§3.2）：「（分叉N）」。持久化数据（同 `NEW_CHAT_TITLE` 的 zh 哨兵
/// 先例：标题是会话数据而非 UI 即时文案）；后缀数字取当前未被占用的最小值 ≥ 2，
/// 重复 fork 先剥掉既有后缀再加新号，不产生「（分叉）（分叉）」套娃。
const FORK_TITLE_SUFFIX_ZH: &str = "（分叉";
/// 英文 UI 时期望的兼容形态：跨语言重复 fork 时也能剥旧号、避让已占号。
const FORK_TITLE_SUFFIX_EN: &str = "(fork ";

/// fork 工作区档位：用户选择隔离的根集合。每个根必须命中源会话钥匙串，
/// 未命中即拒绝（不信任前端路径，防伪造隔离目标）。
#[derive(Debug, Clone, Default)]
pub struct ForkWorkspacePlan {
    pub isolate_roots: Vec<PathBuf>,
}

/// `fork_session` 的结果摘要（命令层回填返回值 + 前端跳转 / 提示用）。
#[derive(Debug, Clone)]
pub struct ForkOutcome {
    pub new_session_id: String,
    pub new_title: String,
    /// 隔离根的 (源路径 → 副本路径) 映射，按隔离顺序；空 = 全共享。
    pub root_map: Vec<(PathBuf, PathBuf)>,
    /// 复制到新会话的消息条数（含注入的提示消息）。
    pub message_count: usize,
}

/// Phase 1 产出的 fork 骨架：切好的前缀、新 id / 标题、解析后的隔离计划。
struct PreparedFork {
    source_id: String,
    new_session_id: String,
    new_title: String,
    /// 前缀消息（可能为空）。
    prefix_messages: Vec<Message>,
    /// 源快照里要继承的标量字段。
    system_prompt: Option<String>,
    context_references: Vec<deepseek_tui::session_manager::SessionContextReference>,
    model: String,
    model_provider: String,
    model_provider_id: Option<String>,
    mode_label: Option<String>,
    /// 待重写的 artifact 元数据。
    artifacts: Vec<ArtifactRecord>,
    /// (隔离源根, 副本目标路径)；共享根不在其中。
    isolation_targets: Vec<(PathBuf, PathBuf)>,
    /// 新会话主根（隔离后翻译；未绑定时为源 metadata.workspace 的展示值）。
    new_primary_workspace: PathBuf,
    /// 钥匙串全量（含主根），隔离根已翻译为副本路径。
    translated_roots: Vec<PathBuf>,
}

fn folded_key(path: &Path) -> String {
    crate::platform::os::filesystem_path_identity_key(&path.to_string_lossy())
        .trim_end_matches('/')
        .to_string()
}

/// 剥掉标题末尾的既有 fork 后缀（（分叉N） / (fork N)），返回基底标题
///（英文形态 `(fork N)` 与基底之间的空格一并去掉，保证跨形态往返一致）。
fn strip_fork_title_suffix(title: &str) -> &str {
    for (opener, closer) in [(FORK_TITLE_SUFFIX_ZH, "）"), (FORK_TITLE_SUFFIX_EN, ")")] {
        let Some(rest) = title.strip_suffix(closer) else {
            continue;
        };
        if let Some(index) = rest.rfind(opener) {
            let digits = &rest[index + opener.len()..];
            if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                return title[..index].trim_end();
            }
        }
    }
    title
}

/// 在既有标题集合中为 `base` 找最小未占用的后缀号（≥2），返回带后缀的新标题。
fn next_fork_title(base: &str, existing_titles: &[String]) -> String {
    let mut used: Vec<u32> = Vec::new();
    for title in existing_titles {
        if strip_fork_title_suffix(title) != base {
            continue;
        }
        for (opener, closer) in [(FORK_TITLE_SUFFIX_ZH, "）"), (FORK_TITLE_SUFFIX_EN, ")")] {
            let Some(rest) = title.strip_suffix(closer) else {
                continue;
            };
            if let Some(index) = rest.rfind(opener) {
                let digits = &rest[index + opener.len()..];
                if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                    if let Ok(number) = digits.parse::<u32>() {
                        used.push(number);
                    }
                }
            }
        }
    }
    let mut next = 2u32;
    while used.contains(&next) {
        next += 1;
    }
    format!("{base}{FORK_TITLE_SUFFIX_ZH}{next}）")
}

/// 隔离副本目标路径（§4.4）：源根同目录 `<name>-fork-<新会话id前4位>`；
/// 极小概率撞名时退化用完整 id，再撞（目标已存在）即报错。
fn isolation_target_for(source_root: &Path, new_session_id: &str) -> Result<PathBuf> {
    let name = source_root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "workspace".to_string());
    let parent = source_root.parent().unwrap_or_else(|| Path::new(""));
    let short = parent.join(format!(
        "{name}-fork-{}",
        new_session_id.chars().take(4).collect::<String>()
    ));
    if !short.exists() {
        return Ok(short);
    }
    let full = parent.join(format!("{name}-fork-{new_session_id}"));
    if !full.exists() {
        return Ok(full);
    }
    bail!(
        "isolation target already exists for {}: {}",
        source_root.display(),
        full.display()
    )
}

/// 重写 artifact 元数据（D9）：`session_id` / `p3art_{id}_` 前缀换成新会话，
/// 指向源会话私有目录（artifacts / workspace）的绝对路径平移到新私有目录
///（内容已随 fork 复制）；其余绝对路径（agent 写进用户工作区的文件）保持
/// 原样——共享根语义下它们仍然有效。
fn rewrite_artifact_records(
    records: &[ArtifactRecord],
    source_id: &str,
    new_id: &str,
    source_private: &Path,
    new_private: &Path,
) -> Vec<ArtifactRecord> {
    records
        .iter()
        .map(|record| {
            let mut rewritten = record.clone();
            rewritten.session_id = new_id.to_string();
            let fabricated_prefix = format!("p3art_{source_id}_");
            if rewritten.id.starts_with(&fabricated_prefix) {
                rewritten.id = format!(
                    "p3art_{new_id}_{}",
                    &rewritten.id[fabricated_prefix.len()..]
                );
            }
            for sub in ["artifacts", "workspace"] {
                let source_prefix = source_private.join(sub);
                if let Ok(rest) = rewritten.storage_path.strip_prefix(&source_prefix) {
                    rewritten.storage_path = new_private.join(sub).join(rest);
                    break;
                }
            }
            rewritten
        })
        .collect()
}

/// 隔离映射提示消息（§4.3）。持久化进新会话开头（普通 user 消息，进入模型
/// 上下文，agent 靠它 + 重新 ls 对齐新路径）；locale 由命令层透传 UI 语言，
/// 未识别时回落中文（与 `NEW_CHAT_TITLE` 哨兵同一取向：会话数据以 zh 为基准，
/// UI 即时文案才走 i18n.js）。注意该消息会被 `is_user_turn_prompt` 计为第 1 轮，
/// 隔离 fork 的后续 rewind / fork 的轮号因此整体 +1。
fn fork_hint_message(translations: &[(PathBuf, PathBuf)], locale: Option<&str>) -> Message {
    let mapping: Vec<String> = translations
        .iter()
        .map(|(from, to)| format!("`{}` → `{}`", from.display(), to.display()))
        .collect();
    let mapping = mapping.join("；");
    let body = match locale {
        Some("en") => format!(
            "(System note) This session was created by forking another one. Isolated workspace mapping: {mapping}. Paths in the history above belong to the original workspaces."
        ),
        Some("ja") => format!(
            "（システム注記）この会話はフォークで作成されました。分離されたワークスペースの対応: {mapping}。履歴内のパスは元のワークスペースを指しています。"
        ),
        _ => format!(
            "（系统注入）本会话由 fork 创建，隔离工作区映射：{mapping}。历史记录中的路径属于原工作区。"
        ),
    };
    Message {
        role: "user".into(),
        content: vec![ContentBlock::Text {
            text: body,
            cache_control: None,
        }],
    }
}

impl SessionStore {
    /// fork 一个会话：复制线性前缀（`keep_turns = None` 全量；`Some(n)` 恰保留
    /// 前 n 个用户 turn，第 n+1 个用户 prompt 为界，含第 n 轮的 assistant /
    /// tool_result）为新会话。源会话只读。`progress` 在目录复制 / 未跟踪文件
    /// 同步期间被调用（计数单调递增），供命令层转发 `fork:progress` 事件。
    pub fn fork_session(
        &self,
        source_id: &str,
        keep_turns: Option<u32>,
        plan: ForkWorkspacePlan,
        locale: Option<String>,
        progress: &dyn Fn(CopyProgress),
    ) -> Result<ForkOutcome> {
        let prepared = {
            let _mutation = self.scheduled_mutation.lock();
            self.prepare_fork(source_id, keep_turns, &plan)?
        };

        // Phase 2：慢 IO 不持 `scheduled_mutation`。isolate_workspace_root
        // 自身保证失败时该根零残留；后续根失败时由下方整体回滚清理先前根。
        let git = GitProgram { program: "git" };
        let mut isolated: Vec<IsolatedRoot> = Vec::new();
        let mut translations: Vec<(PathBuf, PathBuf)> = Vec::new();
        // 任一根的隔离或校验失败：先清掉本轮已成功的全部副本再返回（§6.1 #10）。
        let mut failure: Option<anyhow::Error> = None;
        for (source_root, target) in &prepared.isolation_targets {
            let step = isolation::isolate_workspace_root(
                &git,
                source_root,
                target,
                &prepared.new_session_id,
                progress,
            )
            .and_then(|isolated_root| {
                // §4.4：新路径过 validate_user_workspace_path（存在 + 目录 +
                // Windows verbatim 前缀归一），归一结果作为绑定路径。
                let validated = super::validators::validate_user_workspace_path(
                    &isolated_root.path.to_string_lossy(),
                )
                .with_context(|| {
                    format!(
                        "validate isolated workspace {}",
                        isolated_root.path.display()
                    )
                });
                validated.map(|path| (isolated_root, path))
            });
            match step {
                Ok((isolated_root, validated)) => {
                    translations.push((source_root.clone(), validated));
                    isolated.push(isolated_root);
                }
                Err(error) => {
                    failure = Some(error.context(format!(
                        "isolate workspace root {} for fork of {}",
                        source_root.display(),
                        source_id
                    )));
                    break;
                }
            }
        }
        if let Some(error) = failure {
            for (index, done) in isolated.iter().enumerate() {
                let source = &prepared.isolation_targets[index].0;
                isolation::remove_isolated_root(&git, source, done);
            }
            return Err(error);
        }

        // Phase 3：落盘新会话。失败整体回滚（记录删除在锁外做，delete 自己
        // 会取 `scheduled_mutation`）。
        let commit = (|| -> Result<ForkOutcome> {
            let _mutation = self.scheduled_mutation.lock();
            self.commit_prepared_fork(&prepared, &translations, locale.as_deref(), progress)
        })();
        match commit {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                // 回滚顺序：先删会话记录（若已落），再清隔离副本。删除失败
                // 如实附在错误里（残留半成品比静默谎报成功更可见）。
                let mut rollback_error = None;
                if !self.durable_session_record_is_absent(&prepared.new_session_id) {
                    if let Err(delete_error) = self.delete(&prepared.new_session_id) {
                        rollback_error = Some(delete_error);
                    }
                }
                for (index, done) in isolated.iter().enumerate() {
                    let source = &prepared.isolation_targets[index].0;
                    isolation::remove_isolated_root(&git, source, done);
                }
                if let Some(delete_error) = rollback_error {
                    return Err(error.context(format!(
                        "rollback Session {} also failed: {delete_error:#}",
                        prepared.new_session_id
                    )));
                }
                Err(error)
            }
        }
    }

    /// Phase 1（持 `scheduled_mutation`）：读源快照、切前缀、算标题、解析并
    /// 校验隔离计划。纯读 + 内存构造，无任何写入。
    fn prepare_fork(
        &self,
        source_id: &str,
        keep_turns: Option<u32>,
        plan: &ForkWorkspacePlan,
    ) -> Result<PreparedFork> {
        if self.is_scheduled_session(source_id)? {
            bail!("Cannot fork scheduled-run session '{source_id}'");
        }
        validate_session_id(source_id)?;
        let session = self
            .manager
            .load_session_snapshot(source_id)
            .with_context(|| format!("load_session({source_id}) for fork"))?;

        // 与 rewind / checkpoints 同谓词切前缀（D3）。
        let turn_prompt_indices: Vec<usize> = session
            .messages
            .iter()
            .enumerate()
            .filter(|(_, message)| deepseek_tui::is_user_turn_prompt(message))
            .map(|(index, _)| index)
            .collect();
        let total_turns = turn_prompt_indices.len() as u32;
        let cut = match keep_turns {
            None => session.messages.len(),
            Some(requested) => {
                if requested > total_turns {
                    bail!("会话当前只有 {total_turns} 轮，无法 fork 保留前 {requested} 轮");
                }
                turn_prompt_indices
                    .get(requested as usize)
                    .copied()
                    .unwrap_or(session.messages.len())
            }
        };
        let prefix_messages: Vec<Message> = session.messages[..cut].to_vec();

        // 标题：默认标题的会话保持默认（新会话的自动命名语义照旧）；否则剥
        // 旧后缀 + 最小未占号（§3.2）。
        let existing_titles: Vec<String> = self
            .list_sessions_cached()
            .map(|sessions| {
                sessions
                    .iter()
                    .map(|metadata| metadata.title.clone())
                    .collect()
            })
            .unwrap_or_default();
        let new_title = if session.metadata.title == NEW_CHAT_TITLE {
            NEW_CHAT_TITLE.to_string()
        } else {
            let base = strip_fork_title_suffix(&session.metadata.title).to_string();
            next_fork_title(&base, &existing_titles)
        };

        // 钥匙串解析（§2.3）：绑定会话 = sidecar 钥匙串（cwd-first，含主根；
        // 空 = 单根语义，退化为主根一个开关）；未绑定 = 无隔离可谈。
        let bound_primary = self.session_workspace_binding(source_id);
        let mut keychain: Vec<PathBuf> = Vec::new();
        if let Some(primary) = &bound_primary {
            keychain = self.session_workspace_roots(source_id);
            if keychain.is_empty() {
                keychain = vec![primary.clone()];
            }
        }
        if !plan.isolate_roots.is_empty() && keychain.is_empty() {
            bail!("fork of unbound session '{source_id}' cannot isolate workspace roots");
        }
        // 每个隔离根必须命中钥匙串（折叠身份键比较，防伪造/漂移拼写）。
        let mut isolation_targets: Vec<(PathBuf, PathBuf)> = Vec::new();
        let new_session_id = generate_session_id();
        for isolate_root in &plan.isolate_roots {
            let isolate_key = folded_key(isolate_root);
            if !keychain.iter().any(|root| folded_key(root) == isolate_key) {
                bail!(
                    "fork isolation root {} is not part of session '{}'s workspace keychain",
                    isolate_root.display(),
                    source_id
                );
            }
            let target = isolation_target_for(isolate_root, &new_session_id)?;
            isolation_targets.push((isolate_root.clone(), target));
        }

        // 新主根 / 翻译后的钥匙串：隔离根换副本，共享根保留原路径。
        let translate = |root: &Path| -> PathBuf {
            isolation_targets
                .iter()
                .find(|(source, _)| folded_key(source) == folded_key(root))
                .map(|(_, target)| target.clone())
                .unwrap_or_else(|| root.to_path_buf())
        };
        let new_primary_workspace = bound_primary
            .as_ref()
            .map(|primary| translate(primary))
            .unwrap_or_else(|| session.metadata.workspace.clone());
        let translated_roots: Vec<PathBuf> = keychain.iter().map(|root| translate(root)).collect();

        Ok(PreparedFork {
            source_id: source_id.to_string(),
            new_session_id,
            new_title,
            prefix_messages,
            system_prompt: session.system_prompt.clone(),
            context_references: session.context_references.clone(),
            model: session.metadata.model.clone(),
            model_provider: session.metadata.model_provider.clone(),
            model_provider_id: session.metadata.model_provider_id.clone(),
            mode_label: session.metadata.mode.clone(),
            artifacts: session.artifacts.clone(),
            isolation_targets,
            new_primary_workspace,
            translated_roots,
        })
    }

    /// Phase 3（持 `scheduled_mutation`）：写 sidecar → 复制私有目录内容 →
    /// 落新记录 → 写绑定。任一步失败由调用方整体回滚。
    fn commit_prepared_fork(
        &self,
        prepared: &PreparedFork,
        translations: &[(PathBuf, PathBuf)],
        locale: Option<&str>,
        progress: &dyn Fn(CopyProgress),
    ) -> Result<ForkOutcome> {
        let new_id = prepared.new_session_id.clone();
        let source_private = self.manager.sessions_dir().join(&prepared.source_id);
        let new_private = self.manager.sessions_dir().join(&new_id);

        // 隔离根的路径提示（§4.3 / D10）：以普通用户消息注入新会话开头，
        // 不改写历史消息里的旧绝对路径。
        let mut prefix_messages = Vec::new();
        if !translations.is_empty() {
            prefix_messages.push(fork_hint_message(translations, locale));
        }
        prefix_messages.extend(prepared.prefix_messages.iter().cloned());

        // 模型 / 模式 sidecar 先于会话记录落盘（create_new 同款顺序：写失败
        // 时不留下一个「看似创建成功、重启后回退默认模型」的会话）。
        if let Some(model_id) = self.durable_session_model_id(&prepared.source_id) {
            self.set_session_model_id(&new_id, Some(model_id))
                .context("persist forked session model binding")?;
        }
        if let Some(mode) = self
            .session_mode_states
            .read()
            .get(&prepared.source_id)
            .cloned()
        {
            self.set_mode_and_persist(&new_id, mode)
                .context("persist forked session mode state")?;
        }

        // artifact / ledger 内容随行复制（D9）：会话私有目录下的 artifacts /
        // workspace 两个子目录，存在才复制。
        for sub in ["artifacts", "workspace"] {
            let from = source_private.join(sub);
            let to = new_private.join(sub);
            if from.is_dir() {
                isolation::copy_dir_recursive(&from, &to, progress)
                    .with_context(|| format!("copy session {sub} for fork"))?;
            }
        }

        let mut forked = create_saved_session_with_id_and_mode(
            new_id.clone(),
            &prefix_messages,
            &prepared.model,
            &prepared.new_primary_workspace,
            0,
            None,
            None,
        );
        forked.metadata.title = prepared.new_title.clone();
        forked.metadata.model_provider = prepared.model_provider.clone();
        forked.metadata.model_provider_id = prepared.model_provider_id.clone();
        forked.metadata.mode = prepared.mode_label.clone();
        forked.metadata.workspace = prepared.new_primary_workspace.clone();
        // 血缘元数据（parent_session_id / forked_from_message_count）刻意不写
        //（D1）：当前无消费方，标题后缀承担可见性。
        forked.metadata.workspace_roots = prepared.translated_roots.clone();
        forked.system_prompt = prepared.system_prompt.clone();
        forked.context_references = prepared.context_references.clone();
        forked.artifacts = rewrite_artifact_records(
            &prepared.artifacts,
            &prepared.source_id,
            &new_id,
            &source_private,
            &new_private,
        );

        // 显式持久化路径：不走 update_messages（其截断守卫会拒绝「前缀覆写
        // 空 / 更短消息列表」），守卫本身与其他调用方的保护保持不变——与
        // rewind 的专用路径同一手法。
        let message_count = forked.messages.len();
        self.persist_then_reconcile(&forked, "session fork")
            .context("persist forked session record")?;

        // 绑定 sidecar 要求会话记录已存在（bind_session_workspace_with_roots
        // 的守卫），所以放最后。绑定失败 = 整体回滚（调用方）。
        if !prepared.translated_roots.is_empty() {
            self.bind_session_workspace_with_roots(
                &new_id,
                prepared.new_primary_workspace.clone(),
                prepared.translated_roots.clone(),
            )
            .context("bind forked session workspace keychain")?;
        }

        Ok(ForkOutcome {
            new_session_id: new_id,
            new_title: prepared.new_title.clone(),
            root_map: translations.to_vec(),
            message_count,
        })
    }
}

#[cfg(test)]
mod fork_title_tests {
    use super::{next_fork_title, strip_fork_title_suffix};

    #[test]
    fn strips_both_suffix_forms() {
        assert_eq!(strip_fork_title_suffix("修 bug"), "修 bug");
        assert_eq!(strip_fork_title_suffix("修 bug（分叉2）"), "修 bug");
        assert_eq!(strip_fork_title_suffix("修 bug（分叉12）"), "修 bug");
        assert_eq!(strip_fork_title_suffix("fix bug (fork 3)"), "fix bug");
        // 非后缀形态 / 非数字不动。
        assert_eq!(strip_fork_title_suffix("（分叉2）开头"), "（分叉2）开头");
        assert_eq!(strip_fork_title_suffix("x（分叉a）"), "x（分叉a）");
    }

    #[test]
    fn next_suffix_is_smallest_unused_and_no_nesting() {
        assert_eq!(next_fork_title("修 bug", &[]), "修 bug（分叉2）");
        let taken = vec![
            "修 bug".to_string(),
            "修 bug（分叉2）".to_string(),
            "fix bug (fork 2)".to_string(),
        ];
        // 跨语言已占 2 → 取 3；基底不套娃。
        assert_eq!(next_fork_title("修 bug", &taken), "修 bug（分叉3）");
        assert_eq!(next_fork_title("fix bug", &taken), "fix bug（分叉3）");
        let taken2 = vec!["修 bug（分叉2）".to_string(), "修 bug（分叉3）".to_string()];
        assert_eq!(next_fork_title("修 bug", &taken2), "修 bug（分叉4）");
        // fork 之 fork：源已是「（分叉2）」→ 基底剥后缀，号取未占最小。
        let forked = vec!["源".to_string(), "源（分叉2）".to_string()];
        assert_eq!(
            next_fork_title(strip_fork_title_suffix("源（分叉2）"), &forked),
            "源（分叉3）"
        );
    }
}
