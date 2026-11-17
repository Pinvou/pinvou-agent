# pinvou3 Skill 加载 / 卡牌池 状态

> 留档日期：2026-06-03。验证基线：CodeWhale submodule `0c9542dd`（2026-07-24 重建基线前的旧史哈希；不在 clean 分支线上，但对象在公开仓可解析——2026-06-02 qwen 128K context 提交），pinvou3-app（`card-pool-agency` 分支；§2 Side A 所提 `card-pool` 分支同为内部工作分支，两者均不随本仓库发布）。
> 下面所有 file:line 是写档时的位置，改动后以代码为准。
>
> **2026-11 归档注记**：写档后的重构已使本文多项"当前态"描述失效，§1/§3 请勿按现状使用：
> skill 发现已改为 per-session 组合目录 + materialization（`features/assistant/skill_materialization.rs`、
> `features/assistant/platform/bridge.rs` 的 `explicit_skills_root_only`），「`~/.agents/skills` 残留口子」与
> 「UI 读 bundle / 工具读 ~/.agents 的两套不对称」均不再成立；`list_skills_v2` 已由
> marketplace 接口取代；`pinvou-review-plan/final` 已于 0.7 下线（SKILL.md 删除于 commit
> `9aaff554b`，2026-06-05，其提交说明即「bundle 0.7 清装机残留」；`95502ac81`（2026-08-17，提交处
> VERSION 读 0.8.3、首个收录 tag 为 v0.8.4）为三省六部移除 #285
> （⚠️ `9aaff554b`/`95502ac81` 均为父仓库 pinvou3-app 提交，非 CodeWhale 子模块提交，在子模块对象库中不可解析），
> 仅顺带删除已死的 `WORKFLOW_HIDDEN_SKILLS` 过滤常量）并由 `cleanup_retired_skills`
> 启动清理。§2 卡牌池机制描述仍有效（agency-agents.json 现 268 个 agent）。文中 fork patch
> 序号（#41/#25/#26 等）为重建前私有编号（#432 是底座上游 PR 编号，见 §2 正文），与现行 [`fork-modifications.md`](./fork-modifications.md)
> 登记编号无关。

## TL;DR

- pinvou3 里有**两套互不相干**的机制，别混：
  1. **Skill 加载**（底座 `SkillRegistry` / `load_skill` 工具）—— 渐进式披露，扫目录读 `SKILL.md`。
  2. **卡牌池 / persona 加持**（pinvou3 自建）—— 专家能力档案，**编译进二进制的 JSON**，加持时文本注入。**不是 skill，不扫任何目录。**
- 底座原本扫 ~10 个 skill 目录；pinvou3 **fork patch #41** 已砍到只剩 `~/.agents/skills` + 注入的 bundle 目录。危险的 workspace(=$HOME)相对扫描全部移除。（❌ 本条已失效：见顶部 2026-11 归档注记——现行 skill 发现已改 per-session materialization + `explicit_skills_root_only`。）
- **残留口子**：`~/.agents/skills` 仍被运行时 `load_skill` 工具扫描（本机该目录已存在，目前为空）。（❌ 本条已失效：见顶部 2026-11 归档注记——现行 materialization + `explicit_skills_root_only` 下该口子不再存在。）

---

## 1. Skill 加载（SkillRegistry）

### 实际扫描的目录（patch #41 后）

底座上游 `load_skill`（#432）会扫 workspace 的 `.agents/skills`、`skills`、`.opencode/skills`、`.claude/skills`、`.cursor/skills`、`.codewhale/skills` + home + 全局，共约 10 路径。pinvou3 workspace = `$HOME`，全暴露。

pinvou3 **fork patch #41**（`CodeWhale/crates/tui/src/skills/mod.rs:601` `skills_directories_with_home`）把它砍到只剩：

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

- 数据：`pinvou3-app/src-tauri/resources/common/bundle/personas/agency-agents.json`（Side B，201 个 agency-agents-zh agent，MIT；写档时点值，现 268）。
- 运行时：`include_str!` **编译进二进制**（快照时点为 `personas.rs:18`，现为 `features/personas/mod.rs:25`），首次访问解析进内存 `OnceLock`。**不落盘，~/.pinvou3 里没有**（主语限编译期内嵌 Side B 池；用户自创卡 source="user" 会落盘 `~/.pinvou3/user/personas/<id>.json`，见 `features/personas/mod.rs`），不被任何 SkillRegistry 发现。
- 加持机制：选卡 → `equip_persona` 存 per-session active_persona + pending body → 该 session 首条 chat 一次性 prepend 完整 body（`personas::equip_body_injection`）+ 每 turn 注入轻锚点（`equip_anchor`；KV-cache 口径：锚点逐轮进当轮消息，append-only、每轮活读，不进会话钉住的 system 前缀；body 为该会话一次性进消息）。详见 `features/personas/mod.rs`（快照时点为 `personas.rs`）。
- Side A（card-pool 分支）：pinvou2 agent-market 1078 元数据卡，机制类似但只注入摘要。

---

## 3. 安全态 / 待决

- **已屏蔽**：底座 $HOME 相对的多目录扫描（patch #41）。（❌ 本条已失效：patch #41 系重建前私有补丁编号，该屏蔽机制已被现行 per-session 组合目录 + materialization 取代，见顶部注记。）
- **残留口子**：`~/.agents/skills`。谁往里丢 `SKILL.md`，模型 `load_skill` 工具即可加载，且**不显示在 pinvou3 工作流 UI**。本机该目录已存在（空）。（❌ 本条已失效：见顶部 2026-11 归档注记。）
- **若要 airtight 隔离**：在 `skills_directories_with_home`（mod.rs:601）再去掉 `~/.agents/skills`，只留注入的 bundle 目录（1 行改动）。当前未做（❌ 已失效：现行经 materialization + `explicit_skills_root_only: true` 已收口，见顶部注记）—— 视作"用户跨工具共享 skill"的有意保留。

## 相关

- 现行工具面权威口径：[`tool-governance.md`](./tool-governance.md)
- fork 策略 / patch 登记：`docs/fork-policy.md`、`docs/fork-modifications.md`
- 卡牌池 Side A/B 对照：worktree `card-pool`（元数据卡）vs `card-pool-agency`（全正文）
