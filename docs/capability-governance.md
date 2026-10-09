# 能力治理（Capability Governance）

本文档描述 pinvou3 当前的能力治理架构：哪些能力存在、谁决定它们在某个会话
中可用、运行时如何生效。取代 `tool-governance.md`（v0.9.0 blocklist 时代；
该文件内容尚未全部并入本文，迁移完成前以其为准的部分仍按原文件执行）与
`skill-scope-governance-改动说明.md`（PR 验收记录，文件已随 #287 删除，
内容已沉淀于此）。

> **落地状态**（2026-09-18）：§1、§2 为现状（能力档案已退役，模式能力差量
> 已收敛为静态表 `MODE_TABLE`）；§3 的存储已收敛为**单一 `disabled_bundles.json`**
> （`{scopes, hidden_scopes, default_off_scopes, initialized, project_skills_enabled, plain_defaults_migrated, install_default_synced}`，键 = 包 id，见 §3.2），取代原
> `disabled_connectors.json` + `disabled_skills.json` 双文件与 `skill:` 前缀跨文件借道；
> companion 联动排除改由包模型现算（门控侧解析用 `bundle::skill_gating_owner`，
> 物理嵌套感知，round-26 minor 11 精确化）。§3.1 的
> 统一包模型与「一个包 = 一个开关」已部分落地（`BundleStore` + `bundle_readiness`），
> §3.3 的运行时工具名发现（现为 manifest 预测）、内置 CLI 连接器归并、统一失效入口
> （现为各开关命令分别触发刷新）与 §6 的泛化命令面（现为 `set_disabled_connectors` /
> `set_bundle_visibility` 等；`set_disabled_skills` 已随死测试与死命令清理 PR #540 删除）为**已定方向、未实施**，实施时以本文档为准并更新本注记。

---

## 1. 总览：两条线

```
能力面 = 原生家族线（编译期决策）+ 能力包线（运行期用户开关）
```

| 线 | 管什么 | 决策时机 | 用户开关 |
|---|---|---|---|
| 原生家族线 | 底座 canonical 工具（`bash`/`read`/`write`/`edit`/`list_dir`/`file_search`/`grep_files`/`Git`/`Web`/`agent`/`workflow` 等） | 编译期 | **无** |
| 能力包线 | 一切外部能力：MCP 连接器、组合工具、CLI 连接器、独立技能 | 运行期 | 有（按模式 scope） |

设计纪律：**底座能力是产品承诺，不是用户偏好**——不开放用户级开关，避免
"关掉 `read` 后应用坏了"这类 footgun。运行期配置只给真正有运行期写入者
（用户开关）的能力包线；没有写入者的运行期配置只是常量的间接层。

## 2. 原生家族线（编译期）

```
某模式可见集 = PINVOU3_ALLOWED_TOOLS（白名单）− MODE_TABLE[模式].unavailable_tools
```

- **白名单**（`features/assistant/tool_policy.rs` 的 `PINVOU3_ALLOWED_TOOLS`）：
  产品级安全决策，spawn 时经底座 `allowed_tools` 同时约束首轮目录、
  `tool_search` 结果与实际执行（deny 优先于 allow）。改动需 respawn，
  属安全评审事项。
- **模式能力静态表 `MODE_TABLE`**（`features/assistant/session_policy.rs`）：
  每模式一行 `ModeCapabilities { unavailable_tools,
  skills_empty_hides_load_skill }`，编译期常量、
  查表取数（取代散落的 match 臂），新增模式漏填由穷尽性测试兜底
  （测试遍历的 `SessionMode::ALL` 由编译期穷尽哨兵绑定到枚举变体，
  漏挂即编译失败）。
  语义全部是"该模式架构上有/无此能力"（如 code 的
  `mcp_pinvou3_present_artifact`：产物卡在代码车道没有 UI 消费者），
  不是"默认关掉"——不出现在任何开关面。
- 能力档案（`capability-profiles.json` + `capability_profile.rs` 统一解析器）
  已退役：v0.9.5 起基础集由白名单承担，档案只剩 per-mode 差量，而差量
  没有运行期写入者，JSON + 解析器是多余的间接层。plain 曾默认禁 `Git`
  家族，经决策放开。

## 3. 能力包线（运行期）

### 3.1 数据模型：能力包（已定方向、未实施）

> 现状：连接器与技能已收敛为单一 `disabled_bundles.json`（包 id × SessionMode，
> §3.2），companion 技能按包模型归属（门控侧解析用 `bundle::skill_gating_owner`）随所属包整体
> 上下线；下述统一「包」模型（含 `bundle_kind` 推导与「一个包 = 一个开关」）的其余
> 部分（运行时工具名发现、内置 CLI 归并、统一失效入口）为目标设计，实施时以本节为准。

一切外部能力统一建模为**包**，三个部分均可空：

```
Bundle = { id, name, mcp_servers: [], skills: [], cli: [] }
```

- 纯 MCP 包（`servers` 非空）：本地 stdio 型 / 远程 OAuth 型；
- 组合包（`servers` + `skills` 均非空）：MCP 函数 + 使用引导一体；
- CLI 包（`cli` 非空）：飞书/企微/钉钉/tmeet/ima 等内置连接器；
- 纯技能包（仅 `skills`）：市场预置、用户上传、手放技能。

包的**类型不做存储标签**，由内容现算（`bundle_kind` 推导），只用于 UI
徽标与规则查表——存储标签会和事实漂移，且不可信输入（项目技能、上传包）
自报的标签是提权通道，分类事实只由安装/加载层（可信代码）推导。

**一个包 = 一个开关**：包的暴露面（MCP 工具 + 包内技能引导 + CLI 引导）
整体上下线。包内技能（原 companion skill）没有独立开关，可见性唯一跟随
所属包——不存在"引擎在、引导不在"的半截状态。

### 3.2 默认姿态与用户数据

存储：`~/.pinvou3/disabled_bundles.json` 单一文件（包 id × 模式键控 map）：

```json
{ "scopes": { "<mode>": ["<包 id>"] }, "hidden_scopes": { "<mode>": ["<包 id>"] }, "default_off_scopes": { "<mode>": ["<包 id>"] }, "initialized": ["<mode>"], "project_skills_enabled": false, "plain_defaults_migrated": true, "install_default_synced": ["<mode>:<包 id>"] }
```

scope 键即 `SessionMode` 的 kebab-case 名（当前 `plain` / `code`）；
`initialized` 集合取代原 `code_initialized` 布尔。`default_off_scopes`（评审
R11-B2）记录 `scopes` 中由**安装默认**写入（非用户显式关闭）的条目：安装
同步写 stored+本表，用户 disable 只写 stored，composer 整表写只保留本次
**未触碰**（写前写后都在 off）条目的标记：已初始化 scope 按
`previous ∩ new`；**首次写**（uninitialized DenyAll scope，`previous` 为空）
按 round-13 B1 从**写前有效扩集 ∩ 新列表**播种——照字面执行
`previous ∩ new` 会把每个首次默认都变成显式 opt-out、重新打破 round-13 B1
cohort（round-33 minor 7 勘误）。随真正被切换的
条目一起丢弃——它无法区分"谁关的"，只对能归因的条目不越权（round-12 自审）；
批量 enable 的整批判拒只针对 stored 中**不在**本表
的 id——安装默认的关可被用户动作（欢迎卡/场景 opt-in）移除，显式 opt-out
不可。`install_default_synced`（评审 #455 round-31/32）是 `"<mode>:<包 id>"`
同步账本：install/connect/startup 三个同步变体对观察到的每个 DenyAll
scope × 包对各记一行，**scope 仍处于未初始化时同样记账**（round-32 MAJOR
1：fresh home 上的连接发生在 plain 物化之前，不记账则用户的**首次**启用会
在下次启动被回填覆盖）；启动 refresh 只对账本**缺失**的对回填默认关行；用户
enable 移除 stored 行但**保留**账本条目（使 enable 对 refresh 粘滞）；teardown
（`remove_bundle_from_disabled_scopes_exact`）按 scope 键**精确匹配**清除该包
全部条目（round-32 minor 1），重装/重连因此重新同步默认关。首个版本读取时把旧的
`disabled_connectors.json`（连接器 id）与 `disabled_skills.json`（技能 id）迁移合并：
连接器 id 原样进包 id（连接器 id 即包 id），技能 id 经 scope 侧的包 id 归一
（`to_package_id` → `skill_gating_owner`：manifest 认领优先，物理嵌套回退；
已知包 id 由盾牌直通——评审 #455 round-23 MINOR 1，避免同名技能目录劫持
stored 包行）映射到所属包（companion → MCP/CLI 包，独立技能 → 自身），
`skill:` 前缀跨文件借道残留统一剥除。旧文件本版本内保留为惰性历史（只读新文件），下个版本周期随旧布局退役。
旧文件**存在但不可消费**（读失败、损坏 JSON、形状非法/字段类型错误）时按严格解析判损坏而非
「什么都没关」，走与统一文件损坏恢复同方向的 fail-closed：只冻结迁移标记、不初始化任何 scope
（DenyAll 兜底保住丢失的显式禁用），旧文件原样保留供手工恢复——否则宽升级信号会把升级装机
误初始化成 plain 空 = 全开并在首读永久冻结（评审 #455 round-34，恢复 main 严格解析语义）。
升级时点的 legacy cohort 有一个**已披露的不对称**（评审 #455 round-32
minor 7）：升级时在线的内置 CLI 连接器在首次启动由账本化 refresh 回填为
默认关（连接器有启动刷新臂），而同样在线的 legacy MCP/技能包则**原地保留
live**——零 stored 行、零账本条目，也没有任何启动回填臂；同一"升级时在线"
cohort 得到相反的默认姿态，此句即为二者的登记。

`hidden_scopes`（可见性）与 `scopes`（disabled，开关）是两套**正交**门控
（`marketplace/scope.rs`）：

- 两者都从模型的可调用供给/执行面排除，按并集 `unavailable = disabled ∪ hidden`
  现算（`unavailable_bundles_for`；物化侧同口径，见 `skill_materialization.rs`；
  turn 快照 enabled 口径见 `mcp_inventory.rs`，工具白名单通道
  `unavailable_tool_names_for` 同口径）；
  已安装但禁用的市场 MCP 包会例外地向原生 Engine 会话公开 id、名称与 enabled
  元数据，但不公开其工具 schema，也不恢复工具调用能力（见
  `marketplace-unification.md` §5.4）；
- hidden 只决定包是否出现在 composer 列表，不决定 on/off；disabled 只决定
  开关态，不影响列表可见性；
- 卸载清理走 **exact 形态** `remove_bundle_from_disabled_scopes_exact`（属主在拆除前快照，round-28 与实现对齐），同时清 disabled 与 hidden
  （防残留 hidden 误隐藏未来同名重装；ima 断开随技能卸载走同一入口）；
  CLI 连接器「断开」（logout，删授权不删记录）不走该入口，两个集合均不动；
- 能力开关写路径（`save_disabled_bundles_for`）只写 `scopes`，不动 hidden；
- 连接器开关（`set_disabled_connectors`）复用同一写路径：按 scope 整表重写
  disabled 集（关闭写入、开启移除），两个方向都不动 hidden，也不经过卸载
  清理入口——被 `set_bundle_visibility` 显式隐藏的包，开关开回后仍不可见；
  批量开启入口（`enable_packages_in_scope`，欢迎卡/场景的
  `enable_marketplace_packages`）例外：未初始化 scope 物化「现算扩集 − 请求
  id」，已初始化 scope 从落盘列表移除，并连带清 hidden（隐藏包即使开关打开也
  看不到工具）；用户显式关掉的 id（非安装默认）整批拒绝、不改状态；
- 回收站恢复过**恢复同意门**：恢复的包在已初始化 scope 重新落回默认禁用
  （带安装默认标记，欢迎卡/场景 opt-in 可抬起）；**声明凭据的 MCP 包**
  （manifest `secret_env`/`secret_headers` 任一非空、`config_fields` 含
  `secret: true` 条目、或敏感命名的历史 `env` 键（`is_sensitive_key_name`，
  round-28 补第四腿）——不限于技能组合包；bin 侧 manifest 副本不可读时同向
  强制；与 recycle_bin 实现、marketplace-unification.md 对齐）在未初始化
  scope 走强制变体物化同一门（防供给面零同意上线）；
  门持久化失败
  在消费回收站条目之前报错，恢复可重试。注意两个上报信号的方向（round-24
  MAJOR 5 文档勘误，此前一句写反）：`blocked` 整批判拒只对**已初始化** scope
  的落盘 opt-out 有定义（未初始化 scope 物化的是现算扩集，不存在可对抗的
  落盘行）；`not_applied` 则**只由未初始化的现算扩集臂产生**——请求 id 不在
  扩集中即上报；**已初始化** scope 没有等价信号，恒返回空（未知 id 视为
  已开启且不上报——空 `not_applied` 在该状态下不是覆盖证明）。

**Cross-process consistency (#515)**: the GUI and headless hosts may share one
`~/.pinvou3`; this file's read-modify-write is serialized by an in-process
mutex plus an OS file lock (`~/.pinvou3/disabled_bundles.lock`, fd-lock), so
two concurrent processes can no longer drop each other's updates in a
read-modify-write race (the lost side is the user's explicit off — the
fail-open direction). The failure semantics are conservative throughout: when
the lock is unavailable or the write to disk fails, the write is **refused**
with an error — from the caller's perspective "`Ok` means the change landed",
but the following designed no-op paths also return `Ok` (the known-bundle
registration skip and its ledger variant, same-value toggle short-circuits,
and cleanup/sync finding no change against the current set); they make no
landed-on-disk claim.
On the write side there is **no bounded wait** for an established file lock (a
dying peer process releases the lock with its handle; only a frozen peer
blocks indefinitely; every write entry point runs off the executor, and the
residue sweep and repair writes inside the single-threaded startup window may
wait the same way — a documented tradeoff); policy reads only try the locks
and degrade under contention to an unlocked, never-persisting snapshot
(engine-side per-turn reads never block on a peer; "never persisting" means no
policy state is written — a degraded read at most creates the empty lock file
itself on demand); the opens of both the data file and the lock file are
hardened (round-18/20/21: on Unix `O_NOFOLLOW`/`O_NONBLOCK`, plus a
regular-file fstat gate before any byte **on the data-file opens**, so
FIFOs/devices/symlinks are refused there; the **lock-file** open has no
regular-file gate — a planted FIFO that survives the open is locked on its
own inode where flock works and fails flock where it does not, a deliberate
platform-split outcome documented in platform/filesystem; Windows has no
equivalent and keeps the documented profile-ACL residual — for Windows
behavior see platform/filesystem's module docs); the hardening guards the
final path component only — parent-directory symlinks are still followed,
the same exposure as the rest of the private home's file access — the
cost is that a legitimate dotfile/sync setup that makes
`disabled_bundles.json` a symlink is treated as "unreadable" (the in-memory
over-refusal direction), and the next write renames the symlink aside into a
preserved `.unreadable.<timestamp>` copy and recovers with a regular file;
when the file is corrupt (a parse failure or invalid UTF-8), the locked path
quarantines the corrupt bytes to a backup (`*.corrupt.<timestamp>`; written
only when no sibling copy exists — with one present no second backup is made,
i.e. only the **first** instance of a corrupt file's original bytes is
preserved, and deleting the copy re-arms quarantine). When sibling evidence
exists (either a `.corrupt.*` quarantine backup or an `.unreadable.*`
preserved copy — both count), the NotFound recovery branch deliberately skips
the legacy migration (the sibling evidence proves the unified-store era
existed and the old migration source is an even older snapshot); letting a
re-import proceed would resurrect that stale snapshot (including its stale
initialized marks) as the authoritative state and silently undo every disable
entry recorded after the original migration. From then on the DenyAll scope
is re-derived from the real migration defaults (uninitialized means "every
installed pack off by default", fail-closed), and the original bytes stay
preserved in the quarantine file. During a corrupt/unreadable window,
unlocked policy reads uniformly return that over-deny guess (the disabled set
covering every known pack id; the hidden set likewise emptied and
`project_skills_enabled` likewise treated as off — the composer list is
therefore affected beyond the deny decision). Locked write entry points never
persist the guess: the load arm first quarantines the corrupt bytes and
recovers the fail-closed state from migration defaults, and writes land on
the recovered state (same-value toggle short-circuits etc. still return
normally against the recovered state; no "guess persistence"); the exact
cleanup channel's "no change means no action" rule for an unparseable state
still refuses explicitly. When the lock itself is unavailable, every write
entry point refuses (the error names `disabled_bundles.lock`); these states
last only as long as the window. Three disclosed boundaries: (1) what the
lock serializes is only the on-disk read-modify-write, not the UI snapshot —
the connector toggle rewrites the whole per-scope table, and the submitted
list comes from the snapshot taken when the composer page loaded; another
process sharing the same home that registers deny-first entries for the same
scope between the snapshot and the commit gets them overwritten by the
whole-table rewrite (the user's own last-writer-wins surface; the overwrite
does **not** self-heal — once registered the pack is "known", and while it
stays installed no channel re-registers it, so the exposure lasts until the
user toggles the pack by hand; uninstall→reinstall of the same id is the
exception channel — uninstall clears the record, the entries, and the
materialized connector companion directories (round-17 review), and
reinstall registers afresh like a new install); (2) serialization requires
every process sharing the home to run this version — binaries older than
#515 do unlocked read-modify-writes, so during the upgrade window "old host +
new GUI" still reproduces lost updates (the degraded direction is harmless:
the lock file holds no data and old versions ignore every new sibling file);
(3) the lock covers only this file — the other state files concurrently
read-modify-written in the same two-process scenario (`installed.json`,
`bundles.json`, `mcp.json`, `recycle-bin.json`) serialize on their own
per-file OS locks since main's #656 (`file_lock.rs`, closing #521), so the
lost-update family this section's fix closed for `disabled_bundles.json`
cannot reproduce there; the remaining same-family boundary is structural —
the funnel is a per-file helper family rather than one shared platform-layer
primitive, registered as follow-up work.
Boundary (3)'s import family is likewise closed (round-20/21): the per-id
landing lease gives the import pipeline's staged `bundles/<id>.tmp`, backup
`bundles/<id>.old`, and registration write real cross-process mutual
exclusion — same-id imports, the uninstall command, the retired-tool startup
sweep, and the recycle-bin restore all serialize on
`marketplace/import_journal/<id>.landing.lock`, and the boot reconcile probes
the lease non-blockingly and defers contended entries (the landing-journal
section below documents the arms). What deliberately remains open in this
family: a persistent rename failure in the import pipeline's own rollback
paths (landing failure, supply-failure rollback) can strand content in
`<id>.old` after the journal mark has cleared — logged loudly, and invisible
to the reconcile (which only scans marks), so recovery is manual; this is
the rollback form of the crash-strand residue disclosed with the reconcile
arms below. Two more same-family boundaries: flock provides no cross-host exclusion
— multiple hosts sharing one network-mounted home are outside this module's
threat model (for lock semantics on NFS-like filesystems see the lock-file
line's note); the single-instance constraint is per user — when different
users share one home, the second user gets write refusals/read degradation
from the lock file's 0600 private mode (also the fail-closed direction;
round-17 review: the effective mode is the lock file's own 0600, not the home
root's 0700). The global order among the file locks is likewise unsettled:
the pre-existing ABBA crossing between the transaction lock and the per-id
import lock (including one same-instance reverse theoretical window) is
recorded in the comment at `MARKETPLACE_TRANSACTION_LOCK`; this section must
not be cited as proof of a settled global lock order.

Install/import paths follow a **transaction boundary** in the same direction
(#517 review): DenyAll disabled-set registration happens BEFORE any content
lands or replaces an existing install (deny-first) — when registration is
refused, the whole install/import aborts before landing, never exposing an
"installed but not in the disabled set" intermediate state and never letting
the rollback uninstall destroy an existing user install (a reinstall/re-import
overwrites the old copy in place, and a post-hoc rollback cannot restore it);
the CLI connector enablement (`*_apply_skills` and the auth-gated refresh /
startup backfill's skill materialization) likewise registers the DenyAll
disabled set first and materializes skill files second, and a refusal aborts
the connect before any skill materialization; recycle-bin restore is
deny-first the same way (uninstall already cleared the old disabled entries,
so restore is, in governance terms, "a not-installed pack becoming available
again", not "returning to the pre-uninstall state") — when the destination
already exists, restore refuses in front of the consent gate (a stale recycle
entry must not be able, as the side effect of one restore destined to fail,
to re-disable an install that was reinstalled and re-enabled); registration
precedes any content move-back/landing, so a refusal leaves the pack in the
recycle bin intact and retryable, and a successfully restored pack stays off
by default in an initialized DenyAll scope (an uninitialized DenyAll scope is
backstopped by "every installed pack off by default"). Restore registration
runs **unconditionally**, without an "already installed → skip" check: the
uninstall side's `bundles.json` mirror deletion only logs on failure while
the recycle still continues, so a stale `installed=true` record may survive —
if it participated in the skip check, the restored pack would land without
its disabled entries (exactly the exposure this gate prevents); the reinstall
channel's "known" check no longer blindly trusts that record either: the
store-record clause already requires content corroboration (the pack
directory exists, or an installed skill maps to the record), so a surviving
stale record biases toward "unknown" (the over-refusal registration
direction), aligned with the restore channel's unconditional registration;
unconditional registration is idempotent over surviving residue entries. The
`Builtin` registration record written at connect time does **not**
participate in the "already installed → skip" check (it is written before the
gate runs; counting it as known would skip the entire registration on first
enablement), so a connector's first visibility always registers; once all
companion skill directories have landed, the connector hits "known" (those
skills are neither presets nor uploads, so owner-claim can never vouch for
the connector; a full landing can only come from an earlier gated show, or be
inherited verbatim from a pre-lock older version — at that time "the visible
skills of connected connectors" was the recorded status quo; a partial
landing is treated as "unknown", the over-refusal registration direction)
and keeps the user's recorded authorization state — including every startup's
auth-gated refresh; hide and re-show: the CLI connector hide/logout path does
no teardown and the sync-ledger entry survives, so re-show is a no-op — the
recorded authorization rows are kept verbatim, re-materialization stays
governed by them, and they are never re-denied (round-17 review: the earlier
"re-show after disconnect→reconnect re-registers" described a teardown path
this connector class does not have). Re-registration happens only after a
ledger-clearing teardown: the ima logout (`uninstall_and_strip_scope`), or a
pack uninstall — the latter removes the materialized companion directories
along with the pack and aborts the whole uninstall if that removal fails
(transaction rollback, mirror registration write-back, authorization rows and
ledger kept verbatim, left for the user to retry — the state is kept as-is,
retry safe; the startup retry applies only to the retired-tool sweep leg;
round-18/19). The materialized-"known" clause also requires the sync-ledger
entry to exist (round-18): the ledger entry is written by the gated pass
before materialization and cleared only by ledger-clearing teardowns, so
"directory present while the ledger is cleared" is exactly the post-teardown
state — a gated show that completes materialization mid-teardown across
processes (the per-id import lock is in-process and cannot serialize a
cross-process show/uninstall pair) re-registers in its post-materialization
belt-and-braces sync, and the re-landed directory cannot vouch for the
already-cleared authorization state; the residue is the microsecond-scale
check-then-act between that sync's known check and its registration (same
disclosed family). The CLI connector identity vocabulary is case-folded at
the import guard: a pack id case-insensitively equal to a builtin CLI id is
refused at import, and companion-name claims are folded through
`cli_bundle_of_skill`; the materialization probe and the startup visibility
cache share a `read_dir` exact-name check, so a case-variant directory counts
only as a partial landing (the over-refusal registration direction) and never
as authorized — a planted variant on default macOS/Windows filesystems
therefore biases to over-refusal and converges once the variant pack is
uninstalled (the next gated show re-materializes under the canonical name);
on Linux that exact check behaves identically to the original-path check.
Deny-first applies only to **newly installed** pack ids: a reinstall/update/
re-import of an installed pack skips registration (`Ok`, no write) — the
existing entries of an initialized scope are the user's authorization record;
re-registering would both reset the user's explicit on on the success path
and, on any failure after the gate (pip dependencies, disk full, content
conflict, remote verification), silently disable an otherwise working install
with no recovery path (the same contract as `update_marketplace_skill`'s
"update keeps the user's enabled state"); the lazy disabled entries a failed
new-install leaves behind still point fail-closed and converge at the next
successful install. The uninstall side clears scope entries per channel: in
the MCP tool channel, the strip of **the tool's own id and the companion
skill entries of whole-pack Upload recycles** happens inside the transaction
lock (the cleanup presumes the registrations this call actually removed);
non-Upload combo packs' companion skill entries are cleared row by row with
each skill teardown before the transaction starts (that leg holds the same
round-12 B2 window — the strip runs after the corresponding teardown returns
and outside any lock; an existing shape, disclosed; this fix also removed
main's leftover post-transaction second strip on that channel, collapsing
that leg's window to one hop after teardown); the ima disconnect channel goes
through `uninstall_and_strip_scope` (`ima_logout` has moved to the same entry
point) holding the per-id import lock across both the teardown and the scope
cleanup, and the strip likewise completes inside the lock; the skill
uninstall channel snapshots the owner before deletion (round-26: post-deletion
normalization could be hijacked by a foreign claim) and clears rows exactly,
after the `Ok(false)` no-op exemption — the strip runs after the uninstall
returns and outside the import lock (the round-12 B2 window is still open
here, a disclosed residue; the ima channel is unaffected). The preset skill
install channel (`SkillMarketplaceManager::install`) likewise holds the
per-id import lock across the whole landing (staged unpack → delete-and-swap
→ backup sweep → mirror registration) (round-20), so a same-id install and an
uninstall/disconnect teardown can no longer interleave — the residue
collapses to one hop between the install channel's gate check / pre-steps
(which read the store outside the lock; an ima reconnect includes credential
writes) and taking the lock (same-family window, millisecond scale): when the
gate check reads "known" before the teardown but the landing completes after
it, the pack sits with zero consent rows until its next teardown (disclosed
family; the fail-open direction is exactly this one window).
Across the channels, a same-id deny-first registration that happens inside a
critical section, in the window between the record/teardown commit and the
strip, can still be cleared by that strip (microsecond-scale, fail-open,
disclosed); a registration made after the critical section ends survives
necessarily, except for the skill channel, the non-Upload companion leg, the
install-rollback leg in the next sentence, and the retired-tool startup
cleanup leg (round-18: its no-op-retry leg's landing-marker probe runs
outside the scope lock — the strip body itself is inside the scope lock; when
a same-id import landing marker is present the whole leg is deferred, and
since round-20 the probe point and the strip body hold the landing lease
together, so a same-id import cannot register between them — that leg has no
residual window left). The install-rollback leg (clearing consent rows when
the network verification fails after a successful install) also clears rows
exactly, outside the lock, with the same window as above, and only clears the
rows this install's own deny-first write created — a concurrent same-id
registration can be cleared by mistake; a mistaken clear does not self-heal
until the pack's next teardown passes the gate again (disclosed). The
retired-tool startup cleanup leg (round-17 fix) branches on the uninstall
outcome: when the uninstall rolls back, the pack is still registered and its
disabled/hidden rows are current consent — kept, not cleared; only the
success and no-op-retry legs clean up; while a same-id import is landing (the
round-20 fix) the entire cleanup — the strip and the directory-deletion leg
alike — is deferred to the next startup: the probe point is guarded by the
landing lease (see below) together with the landing marker, so the import's
landing→registration gap no longer has a deletion leg guarded only by record
probing. The user uninstall command holds the same landing lease across its
whole span (the round-20 fix): either the same-id import's gate runs before
the uninstall and its rows are cleaned together with the records this
uninstall removes, or the gate runs after the uninstall, re-registers as a
new install, and the landing is governed — the cross-process window "import
passes the gate (skips registration) → uninstall commits and clears rows →
import lands ungoverned" on the tool/pack channel is closed. The remaining
residue is the same-family microsecond check-then-act: the skip check reads
the store outside the scope lock (same-id reinstall vs uninstall races; the
tool/pack channel is closed by the landing lease, and the skill/ima channels'
in-lock interleaving was closed by install holding the lock — the residue is
one hop between the install channel's gate check / pre-steps (reading the
store outside the lock; an ima reconnect includes credential writes) and
taking the lock, plus the two channels' cross-process two-state cases: the
import lock is in-process and the skill channel holds no cross-process
lease), and between the probe points and the strip bodies of the strips that
run outside locks (the install-rollback leg, the skill command channel) — all
same-id, narrow, bounded; the reverse outcome — leaving a disabled entry
behind for an uninstalled id — is fail-closed and converges. Round-21 fix:
the preset skill install/update pipeline (unpack, fingerprint,
replace-on-disk, registration — the whole span) now holds the same per-id
in-process import lock as uninstall/show-edit/unified import — previously
that channel held no lock, and the in-process interleave "install passes the
gate (skips registration) → uninstall commits and clears rows → install lands
and revives" left a persistent fail-open ("directory + record both present,
consent rows already cleared"; the known-clause vouches forever). The
folded-detour residue previously listed alongside it was closed by the import
pipeline's owner-claim-divergence refusal: a new import whose pack id, after
`to_package_id` folding, lands on another installed claimant (a companion
skill name, a CLI companion directory, `ima-skills`) is refused in the
pipeline; with no claimant present the id maps to itself and stays
importable (the export→re-import loop contract unchanged); old packs with
such ids (imported before this fix) may still exist: they are refused at
import (with a rename-retry hint); restoring an old pack containing a
same-named skill directory is refused the same way (hinting to uninstall the
claimant pack first) — a pure-MCP old pack (no `skills/` directory) is not
refused at restore; its pack row is stored verbatim and self-maps on the read
side after landing, the fail-closed direction. Self-mapping residue
(disclosed in round-15): when a preset skill name is not occupied by the
preset, a user-uploaded pack can take that id (self-mapping, a compliant
import, registered normally under deny-first); when the preset skill is then
installed, its consent gate hits "known" and skips (the upload record and the
content directory corroborate), and the preset inherits the id's existing
entries instead of re-registering — if the user had explicitly enabled the
uploaded pack, the preset lands "enabled" (that authorization was not made
for the preset); if it was never touched, what is inherited is exactly the
default-off row deny-first would have written, so the state is identical.
The gate cannot distinguish, in this context, "an upload with the same id"
from "a historical install of that preset"; the install sweep may also
consume the uploaded pack's directory. Uninstall-reinstall or an explicit
toggle of that id returns the state to unambiguous. Combo-pack imports whose
skill components are not declared in the MCP manifest's `companion_skills`
are registered individually per component pack under deny-first (bounded
inside an initialized scope); an uninitialized DenyAll scope is not a gap:
its default set is also derived, as a union, through the physical-owner
mapping (`skill_gating_owner_with`) over a `bundles/*/skills/` disk
enumeration, so a landed independent component resolves to its owning pack id
and falls inside the default-all-off (round-17 review: the earlier residual
description "the default set is derived only from preset + upload
registrations, so components stay uncovered by the default-all-off backstop"
did not match the code and is hereby corrected).

**Import landing journal (introduced round-18/19, disclosed round-20)**: the
unified import pipeline writes a landing marker
(`marketplace/import_journal/<id>.pending`) before the deny-first gate, and a
guard clears it when the call exits (a gate refusal, a supply-failure
rollback, a rename failure); only "process death between the marker and the
exit" leaves the marker on disk, for every startup's
`reconcile_import_journal` to converge. The marker itself is only a hint — it
cannot distinguish an in-flight import from a crashed one — the real
liveness signal is the landing lease (introduced round-20): before writing
the marker the pipeline takes the cross-process file lock on
`marketplace/import_journal/<id>.landing.lock` (fd-lock, the same primitive
as `disabled_bundles.lock`) and holds it until after the landing guard
clears the marker; same-id cross-process imports therefore serialize on the
lease (the shared staged `.tmp`/`.old` paths thereby gain real cross-process
mutual exclusion — previously only in-process), and the user uninstall
command, the retired-tool startup sweep, and the recycle-bin restore
(round-21: restore is a same-id `bundles/<id>` writer — without the lease,
the sweep's "record probe → directory delete" pair can delete the
just-restored only copy in its take_back→registration gap, and a same-id
import can replace the restored directory wholesale) each take the same
lease, blocking or non-blocking respectively (when the sweep hits contention
it defers wholly to the next startup, and the directory-deletion leg is no
longer guarded by record probing alone; the blocking takers run in
spawn_blocking and their span is local work only — staging, rename,
registry, keyring deletes — so a peer import holds a same-id op for
seconds at most, and a system-keychain prompt inside the uninstall
span stalls peer same-id ops for the prompt's duration, the same
accepted class as the scope lock's frozen-peer wait). The four startup-convergence arms:
lease held (in-flight) → skip the whole thing, retry next startup; marker +
no pack directory → sweep the staged `<id>.tmp` and clear the marker; marker
+ a registered record → sweep the crashed re-import's staged `<id>.tmp`
(round-21: if the sweep fails, keep the marker and retry next startup), then
clear only the marker; marker + a landed directory with no record → move the
directory into the `import_journal/<id>.crash-<timestamp>` holding area
(manual recovery), then clear the marker. Disclosed residual: a crash
between the `install_upload` transaction commit and the bundles.json mirror
landing falls into the fourth arm — the directory is quarantined while
`installed.json`/`mcp.json` are already committed, leaving a dead engine
entry pointing at the moved directory (the fail-closed direction; no active
scan will mistakenly adopt it, and a manual uninstall cleans it up; the
marker itself is not lost). The lease/marker files hold no user data and can
be deleted while the app is closed; hand-deleting an active import's lease
file amounts to giving up that id's cross-process mutual exclusion.

每个模式的默认策略显式声明为**模式身份**（`core/session_mode.rs` 的
`SessionMode::pack_default_policy()`），不再是存储层的硬编码分支：

| 模式 | 包默认策略 | 含义 |
|---|---|---|
| plain | **DenyAll** | 全禁（工具开关全量收敛：外部能力一律显式开启） |
| code | **DenyAll** | 全禁（外部能力显式开启，封泄露面/攻击面） |

`PackDefaultPolicy::AllowAll` 变体已随收敛退役（仅保留枚举形态）。
重新引入的判据：新模式必须在其模式文档中论证「默认放行外部能力」的
同意模型（对齐本文件 §3.2 的显式开启原则），并给出该模式 DenyAll 化的
迁移路径；未经此论证不得恢复任何模式的 AllowAll 默认。

plain 从 AllowAll 翻为 DenyAll 时的**存量迁移**（读时迁移，见
`scope.rs::load_disabled_bundles_file_locked`）：旧版文件（无
`plain_defaults_migrated` 字段）或旧双文件时代的装机，plain 被初始化为
落盘列表——锁定升级前 AllowAll 语义下的真实开关状态（缺省空 = 全开），
升级后用户无感；全新装机只置迁移标记不初始化，未初始化 plain 按 DenyAll
兜底（默认全关）。「升级 vs 全新」的判定使用**宽口径升级信号**：三份开关
相关文件皆无、但 `marketplace/installed.json` 或非空 `sessions/` 目录存在
即视为升级装机——统一文件自 v0.8.6 起就存在且只在有内容可写时才落盘，
老装机 + 从未动过开关的用户可能三者皆无。信号只检查这两条特定路径：家目录
即使持有无关状态（`knowledge/`、`logs/` 等），只要二者皆无仍判全新。
`settings.json` **不构成**信号（评审 #455 R8-3）：预置模板/跨机拷贝的
settings.json 会把全新装机误判为升级（plain 全开，fail-open）。收窄**并非
无遗漏**（评审 R11-M4）：真实老装机通常留有非空 `sessions/`，但被工具清空
`sessions/`、又无 `installed.json` 与 legacy 文件的老装机会被误判全新——
方向是 fail-closed（默认全关，可用性而非安全问题）；两类群体在无持久版本
标记时不可区分（见 follow-up 注册表）。

已知限制（进程内备忘的时效性，评审 #455 R9/R11）：freeze 落盘失败
（`UNPERSISTED_VERDICT`）与损坏恢复覆盖写失败（`PENDING_CORRUPT_RECOVERY`）
各有一个进程内备忘，命中即复用、不重复落盘尝试；任意一次成功落盘会清除
对应备忘（文件回到合法 JSON）。备忘不跨进程持久——**重启后**若磁盘故障
仍未恢复，首读会重新走对应分支。两个分支的重启方向**不同**（评审 R11-M4
如实化）：损坏恢复的重跑是 fail-closed（重新隔离一次、内存全关兜底）；
freeze 的重跑是 **fail-open**——重启后首读会用已存在的 `sessions/default`
重新判定，全新装机被误判为升级 ⇒ plain 全开（正是 freeze 要防的翻转；
进程内备忘只覆盖单次生命周期，跨重启的持久化即登记的
crash-during-freeze durability follow-up，根治靠 settings 版本标记）。该信号会被应用自身首启行为污染（bridge
boot 自写 `sessions/` 目录项、缺省补写默认 `settings.json`），因此首读被
上提至各宿主启动钩顶部（GUI setup、headless bridge、dump_system_prompt，
早于一切首启自写），且判定在**首次读取时无条件落盘**（置
`plain_defaults_migrated`）冻结——否则全新装机会被自己的首启痕迹误判为
升级装机而翻回全开。文件损坏时不静默覆盖：先隔离为 `.corrupt.<ts>` 副本，
再按 **fail-closed** 一次性恢复落盘——只置迁移标记（冻结为全新装机判定）、
不初始化任何 scope，未初始化 scope 按 DenyAll 兜底（宁可恢复全关，不把
用户显式关过的状态恢复成全开）。隔离写失败时同样不覆盖，并布防
unreadable-original 标记：后续任何写入先 rename-aside 保存仍未隔离的原始
字节再落新状态（round-29 m1，评审 #455）——瞬时差分故障（隔离写失败、
主写后成功）不再销毁唯一副本。

用户数据语义（三条线一致）：

- 某 scope 无记录 → 回落编译期默认（跟随产品演进）；
- 用户首次 toggle 时物化整个 scope 列表落盘 → 此后冻结，默认调整不穿透
  已做过选择的用户（`initialized` 集合标记初始化）。**已知的权衡**：物化
  的触发面比「显式开启」更宽——composer 开关**任一方向**的首次 toggle 都
  走整表回写（纯关闭在语义上是 no-op，但同样固化当前扩集）；场景 opt-in /
  欢迎卡批量开启（`enable_packages_in_scope`）也会物化其请求的那个 scope
  （未初始化时落盘「现算扩集 − 请求 id」）。落盘的是当时的现算扩集（减去被
  开启的 id）——此后应用更新新增的内置包不在落盘列表里，会在该 scope 默认
  **开**。产品接受
  此权衡：做过开关选择的用户视为已关注过该 scope 的外部能力面，新增包默认
  开的暴露与旧 AllowAll 语义相当且范围更窄；若需反向收敛，走后续版本的全量
  默认重置。边缘情形：
  installed.json 读不出时，fail-closed 兜底扩集（全部可装包 ∪ 内置 CLI ∪
  技能 owner 包）会在用户显式开启动作时被整体物化为该 scope 的落盘状态——
  未来新增内置包同样默认开，属上述同一权衡的极端入口；

- 未知条目（工具下架、上游改名残留）静默忽略，写回时清理。

项目级技能（`.agents/skills` 等）保持**独立开关**、默认关：项目内文本是
prompt-injection 面，信任级别与包装机技能不同，开启路径展示注入风险警告。
参与范围**跟绑定不跟模式**：仅绑定了真实目录的会话（原生 code 会话的项目
目录、普通 chat 会话的用户工作目录绑定）在开关开启时扫描项目技能；未绑定
会话（含 code 临时工作区）不扫描。

### 3.3 生效通道

```
开关 → capability_changed(scope, line) → 下一轮生效（不 respawn）：
         持久化 → 重算投影 → 组合目录重写 → 会话 disallowed 热刷 → 事件广播

   包的 MCP 部分:  工具名进 disallowed_tools → catalog retain 过滤
   包的技能部分:   组合目录物化（~/.pinvou3/sessions/<sid>/skills/）
                  → 底座每轮重扫渲染 ## Skills 块
                  → 组合目录为空 → load_skill 一并隐藏（无"假开关"状态）
```

这里的“热刷”只更新当前会话/轮次的目录与最终调用权限，不会全局断开共享
`McpPool` 中已经连接的 server；全局断连会影响仍获授权的其他会话。连接按正常
pool/session 生命周期回收，catalog 与最终 dispatch 都继续 fail closed。

- **会话中关闭的边界（上下文不可撤回）**：`## Skills` 块在系统提示里，底座每轮
  重拼系统提示（发现走 mtime 缓存，组合目录一变下一轮即失效重扫），所以禁用后
  **新轮次**不再列出该技能名、后续 `load_skill` 也读不到正文。但**已进入上下文的
  内容撤不回**——若本会话此前已 `load_skill` 读过该技能，其正文留在消息历史里，
  模型仍记得并可能继续按其引导作答。因此供给层关闭是「体验层尽力而为」（§5.1），
  真正的保证在执行层：execpolicy deny 硬拒该技能目录内脚本的 spawn、disallowed_tools
  过滤 MCP 工具，二者不依赖上下文是否已被污染。纯引导型技能（无脚本）在已被读入
  后无法从模型记忆里擦除，这是 LLM 上下文的固有边界，记录为已知限制。

- **工具名获取以 manifest 预测为主**（现状）：`marketplace/mod.rs` 的
  `model_tool_names` 按 `mcp_{server}_*` 前缀 + manifest 声明工具名生成
  禁用名单；查引擎实际暴露的 `model_name`（运行时发现，底座权威）为
  **已定方向、未实施**——落地后清单错配（如 id 含连字符）才不会导致
  "禁不掉"；
- **唯一失效入口** `capability_changed`（目标形态，未实施；现状为各开关命令分别触发 `refresh_live_sessions_skills` 等刷新）：任何开关不可能漏刷下游；
- spawn 初值与热刷同经 `bridge.shape_disallowed_tools` 按会话整形。

## 4. 模式扩展（新增设计/聊天等模式时）

模式是**编译期封闭集合**（模式身份影响 spawn 时的底座配置，不允许运行期
自定义）。新增一个模式要回答的问题已全部显式化，按清单逐项回答即可：

1. `core/session_mode.rs`：加 `SessionMode` 变体并挂入 `declare_all_modes!`
   列表（编译器守所有穷尽 match；漏挂 ALL 直接编译失败），补
   `as_str`/`from_scope_str`；
2. **安全姿态决策**：`pack_default_policy()` 为该模式选 AllowAll/DenyAll
   ——依据是该模式会话内容是否会出现不受控外部文本（prompt-injection 面），
   这是代码评审级别的决策；
3. `session_policy.rs`：`MODE_TABLE` 加一行，逐格回答——模式缺席工具
   （`unavailable_tools`）、空目录是否隐藏 `load_skill`、是否参与项目级
   skills。漏填由穷尽性测试兜底失败；
4. 存储层**零改动**：scope 键即模式名，用户首次 toggle 时 map 自然多键；
5. 前端：scope 字符串协议不变，新模式名即传新的 kebab-case 名。

已知演进分支（今天不写代码）：若产品要求"不发版热更新模式能力"，模式是
编译期封闭集的前提才被打破，届时模式身份层整体迁运行期配置（带播种与
合并逻辑），属独立立项。另一个待决产品问题：模式增多后 scope 应按模式
键控还是按信任类共享（避免用户为每个模式各拨一遍开关）——留给第一个
真实的新模式落地时回答。

## 5. 底座硬防线（与上层状态无关）

```
白名单 allowed_tools（编译期，spawn 给定）
→ disallowed retain（deny 优先）
→ 执行门逐次校验（catalog 缺席 ⇒ 调用拒绝）
→ 审批分类（McpRead / McpAction 风险分级）
```

UI 或状态层出 bug 也放不出白名单外能力。已知开放侧翼：CLI 包的真实执行
面是经 `bash` 调用 CLI，开关只能隐藏引导；要封死需 bash hook 拦截，
当前作为已接受风险记录于此。

## 6. 前端接线（目标形态，未实施——现状为 `set_disabled_connectors` / `set_bundle_visibility` 等各开关命令 + `remote_control:tools_changed` 事件）

- 命令面：`list_capability_items(scope)` 读全量状态（默认已合并），
  `set_capability_enabled(scope, id, enabled)` 唯一写入口；前端不在 JS 侧
  计算默认、不判断模式差异、不直接读写 JSON；
- 事件面：`capabilities_changed` 广播，各窗口/远程实例监听后重取状态；
- UI：设置页「能力管理」区，plain/code scope 切换 + 能力包分组
  （类型徽标）+ 项目级技能独立开关；界面文案三语走 `shared/i18n.js`。

## 7. 已知限制

#279 遗留、按 `marketplace-unification.md` Phase 4 承诺登记于此：

- **OAuth 远程包 readiness 恒 Ready**：远程 OAuth MCP 包（manifest `servers`
  非空）没有必填凭据声明——`tool_credentials` 收敛全部三路声明
  （`config_fields` 各条目按其 `required` 标记，`secret: false` 也计入；
  `secret_env`；`secret_headers`），不含 `servers`（`bundle.rs`；round-28
  MAJOR 2 勘误：round-27 m8 曾把「仅 secret:true」的 `manifest_secret_targets`
  过滤误归到此函数——那是恢复同意门的凭据探测，另一条收敛路径），而
  `readiness_for` 对 Mcp/Bundle 只查 credentials 必填项
  是否在系统凭据存储，因此远程包恒报 Ready。**无法用 readiness 门控 OAuth
  授权是否完成**；授权态由 `connect`（flow=oauth）流程自理，UI 只能依赖
  `oauth` 标记打徽标，不能给「未授权」态。

另有三条限制已随文或在此登记：

- **丢失存储臂（round-30 m4，评审 #455）**：`installed.json` 被**删除**（非损坏——损坏已
  fail-closed）且 `mcp.json` 仍记有 server 条目时，`read_installed` 把 NotFound 判为
  「确认空注册表」，registry 腿为空而磁盘腿对纯 MCP 包失明 ⇒ 未初始化 plain scope 中
  该包工具被放行。**round-31 m9 更正：该开口与模式无关**（expansion 对
  plain/code 同构，`session_mode.rs`）——未初始化 **code** scope 同样受影响，非
  plain 独有。触发需外部破坏存储文件；与 `sessions/` 信号的丢失处置不对称
  （后者有 `.corrupt.*`/`.unreadable.*` 兄弟证据的 fail-closed 恢复臂，scope.rs
  丢失存储恢复——该分支**刻意跳过 legacy 迁移**，兄弟证据证明统一期存储存在过，
  其搁置的判定不可知，宁全关不翻全开）。收敛方向：NotFound 时从 `mcp.json` 重建
  id，或在此登记为外部破坏下的已知限制（现按后者登记）。§7 清单同时补记
  **companion 技能同意同步失败**（round-31 m9 登记、round-33 MAJOR 1 关闭）：
  install 流的 post-install companion 腿曾按 log-only 吞掉同步失败
  （round-32 minor 12 更正过定位）；round-33 MAJOR 1 后该例外**已关闭**——
  仅当伴随 id 归一化等于工具自身包 id（可证明已被工具级同步覆盖）才跳过，
  其余（known-pack-shield 边缘的真实写入）以 `?` 传播、命令失败且错误文案
  携带前端共享标记 `CONSENT_SYNC_FAILURE_MARKER`；**uninstall 事务内的
  companion 循环自始以 `?` 传播**。
- 会话中关闭的上下文不可撤回边界（§3.3 末）、
  CLI 包真实执行面经 `bash` 的开放侧翼（§5 末）。
- Same family: **hand-placed skills are not gated by deny-first** — skills
  placed by hand under `user/skills/`, and directories under
  `bundles/*/skills/` claimed by no installed pack, have no application-side
  writer to register deny-first entries for them and no composer toggle; they
  are governed only by the owner-claim vocabulary. The two placements'
  exposures differ (round-17 wording fix): `user/skills/` is the only
  placement with zero coverage in **every** scope shape — DenyAll's disk
  backstop walks only `bundles/*/skills/`, and the owner mapping resolves a
  hand-placed skill to itself; whereas unclaimed directories under
  `bundles/*/skills/` are exposed only in initialized scopes **not yet
  initialized by a materializing write** (an uninitialized DenyAll scope's
  on-the-fly expansion covers those directories via the `bundles/*/skills/`
  owner mapping; a scope initialized through the welcome-card/scene opt-in's
  materializing initialization or the secrets-pack restore gate's
  materializing initialization lands those unclaimed directories' owners as
  stored rows too — in steady state that row survives the composer's
  whole-table writes verbatim under the known-pack shield, and only the
  stale-snapshot race between boundary (1)'s whole-table write and the
  materializing-initialization write can overwrite and lose it, see boundary
  (1) above),
  and that exposure does not disappear across scope
  migration.

## 8. 相关文件

| 项 | 位置 |
|---|---|
| 白名单 | `pinvou3-app/src-tauri/src/features/assistant/tool_policy.rs` |
| 模式身份（`SessionMode` / 包默认策略） | `pinvou3-app/src-tauri/src/core/session_mode.rs` |
| 模式能力静态表 `MODE_TABLE` | `pinvou3-app/src-tauri/src/features/assistant/session_policy.rs` |
| 整形聚合点 | `pinvou3-app/src-tauri/src/features/assistant/platform/bridge.rs`（`shape_disallowed_tools`） |
| 禁用名单生成（含通配兜底） | `pinvou3-app/src-tauri/src/features/marketplace/mod.rs`（`model_tool_names`） |
| 组合目录物化 | `pinvou3-app/src-tauri/src/features/assistant/skill_materialization.rs` |
| 开关命令 | `pinvou3-app/src-tauri/src/app/commands/connectors.rs` |
