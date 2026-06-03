# pinvou3 Skill 加载 / 卡牌池 状态

> 留档日期：2026-06-03。验证基线：DeepSeek-TUI submodule `0c9542dd`，pinvou3-app（card-pool-agency 分支）。
> 下面所有 file:line 是写档时的位置，改动后以代码为准。

## TL;DR

- pinvou3 里有**两套互不相干**的机制，别混：
  1. **Skill 加载**（底座 `SkillRegistry` / `load_skill` 工具）—— 渐进式披露，扫目录读 `SKILL.md`。
  2. **卡牌池 / persona 加持**（pinvou3 自建）—— 专家能力档案，**编译进二进制的 JSON**，加持时文本注入。**不是 skill，不扫任何目录。**
- 底座原本扫 ~10 个 skill 目录；pinvou3 **fork patch #41** 已砍到只剩 `~/.agents/skills` + 注入的 bundle 目录。危险的 workspace(=$HOME)相对扫描全部移除。
- **残留口子**：`~/.agents/skills` 仍被运行时 `load_skill` 工具扫描（本机该目录已存在，目前为空）。

---

## 1. Skill 加载（SkillRegistry）

### 实际扫描的目录（patch #41 后）

底座上游 `load_skill`（#432）会扫 workspace 的 `.agents/skills`、`skills`、`.opencode/skills`、`.claude/skills`、`.cursor/skills`、`.codewhale/skills` + home + 全局，共约 10 路径。pinvou3 workspace = `$HOME`，全暴露。

pinvou3 **fork patch #41**（`DeepSeek-TUI/crates/tui/src/skills/mod.rs:601` `skills_directories_with_home`）把它砍到只剩：

| 路径 | 谁读它 | 备注 |
|---|---|---|
| `~/.agents/skills` | 运行时 `load_skill` 工具（`tools/skill.rs:94` `discover_in_workspace`） | patch #41 唯一保留的全局约定 |
| `~/.pinvou3/bundle/skills/<name>/SKILL.md` | pinvou3-app UI（`commands.rs:1090` `list_skills_v2`）+ engine 系统提示 skill 清单（`bridge/mod.rs:413` `EngineConfig.skills_dir`） | pinvou3 注入（patch #25/#26） |

被移除（不再扫）：`.claude/skills`、`.opencode/skills`、`.cursor/skills`、`.codewhale/skills`、`$HOME/skills`、workspace `.agents/skills` 等所有 $HOME 相对路径。

### 当前 bundle 里的 skill

`~/.pinvou3/bundle/skills/`：`pinvou-review-plan`、`pinvou-review-final`（启动时由 `bridge/bundle.rs` 解包；二者在工作流 UI 里被 `WORKFLOW_HIDDEN_SKILLS` 隐藏）。

### 两个不对称（注意）

1. **UI 读 bundle，工具读 ~/.agents** —— `list_skills_v2` 只读 `bundle_skills_dir`；运行时 `load_skill` 只读 `~/.agents/skills`。两边目录不同。
2. pinvou3 自己的 bundle skill **不是**通过底座 `load_skill` 工具给模型的，而是走 pinvou3 自建的 `start_skill_session` 一次性 prepend 注入。底座 `load_skill` 对 pinvou3 而言实际只是 `~/.agents/skills` 的入口。

---

## 2. 卡牌池 / persona（不是 skill）

- 数据：`pinvou3-app/src-tauri/resources/bundle/personas/agency-agents.json`（Side B，201 个 agency-agents-zh agent，MIT）。
- 运行时：`personas.rs:18` `include_str!` **编译进二进制**，首次访问解析进内存 `OnceLock`。**不落盘，~/.pinvou3 里没有**，不被任何 SkillRegistry 发现。
- 加持机制：选卡 → `equip_persona` 存 per-session active_persona + pending body → 该 session 首条 chat 一次性 prepend 完整 body（`personas::equip_body_injection`）+ 每 turn 注入轻锚点（`equip_anchor`）。详见 `personas.rs`。
- Side A（card-pool 分支）：pinvou2 agent-market 1078 元数据卡，机制类似但只注入摘要。

---

## 3. 安全态 / 待决

- **已屏蔽**：底座 $HOME 相对的多目录扫描（patch #41）。✅
- **残留口子**：`~/.agents/skills`。谁往里丢 `SKILL.md`，模型 `load_skill` 工具即可加载，且**不显示在 pinvou3 工作流 UI**。本机该目录已存在（空）。
- **若要 airtight 隔离**：在 `skills_directories_with_home`（mod.rs:601）再去掉 `~/.agents/skills`，只留注入的 bundle 目录（1 行改动）。当前未做 —— 视作"用户跨工具共享 skill"的有意保留。

## 相关

- fork 策略 / patch 登记：`docs/fork-policy.md`、`docs/fork-modifications.md`
- 卡牌池 Side A/B 对照：worktree `card-pool`（元数据卡）vs `card-pool-agency`（全正文）
