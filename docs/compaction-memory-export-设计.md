# 压缩长期记忆导出（Codex 兼容）设计

> 状态：r14 已实现（CodeWhale `e3c57d97` + 父仓配套）。
> 英文摘要见 [`docs/fork-modifications.en.md`](fork-modifications.en.md) 的 r14 小节；
> fork 清单见 [`docs/fork-modifications.md`](fork-modifications.md)。

## 1. 目标与非目标

**目标**：上下文压缩（compaction）发生时，像 Codex 一样生成长期记忆，且文件格式与
openai/codex 的 memories 工作区字节级兼容——Pinvou 导出的文件可以被 Codex、其他工具
或人直接消费。

**非目标**：

- 不复制 Codex 的两阶段后台管线（Phase 1 从 rollout 库提取 + Phase 2 整合代理）。
  我们只在压缩时刻做一次 Phase-1 式提取。
- 不写 `MEMORY.md` / `memory_summary.md`。它们是 Codex Phase-2 整合代理的产物
  （`memory_summary.md` 必须以精确的 `v1` 行开头、按 token 预算重排的忠实摘要），
  机械拼接代写会违反其格式契约。Codex 下一次运行时会基于 git 工作区 diff 把
  我们写入的输入整合进它自己的这两层。
- 不做记忆回读注入（future work：可按 Codex `read_path` 模式注入
  `memory_summary.md`）。

## 2. 落盘位置与格式

记忆根：`~/.pinvou3/memories/`（`paths::long_term_memory_root()`，可用
`PINVOU3_HOME` 整体重定位；CodeWhale 独立运行时默认 `~/.codewhale/memories/`）。
与两个既有记忆完全隔离：应用自有记忆 `~/.pinvou3/user/memory/`，底座原生记忆
`~/.codewhale/memory/`。

产物（字节格式与 `codex-rs/memories/write/src/storage.rs` 对齐）：

```
~/.pinvou3/memories/
├── raw_memories.md          # "# Raw Memories" + 稳定升序 thread-id 的 ## Thread 条目
└── rollout_summaries/
    └── <ts>-<hash>[-<slug>].md   # thread_id/updated_at/rollout_path/cwd[/git_branch] 头 + 任务式复盘正文
```

`## Thread` 条目头四行固定为 `updated_at`（RFC 3339）、`cwd`、`rollout_path`
（= `~/.pinvou3/sessions/<thread_id>.json`，Codex 溯源字段）、`rollout_summary_file`。
文件名 stem 算法从 Codex 逐行移植：UUID v7 优先取内嵌时间戳，非 UUID 回退
FNV-式哈希种子；短哈希为低 32 位模 62⁴ 的 4 字符 base62；slug 清洗为小写、
非字母数字转 `_`、截断 60 字符、去尾 `_`。测试用上游文档中的 v7 示例 UUID
锁定到具体 stem `2026-02-18T00-30-34-JjOz-fix_auth_flow`。

正文两层都由单次 LLM 提取调用产出（浓缩自 Codex `stage_one_system.md`，
STRICT schema 保持一致）：

- `raw_memory`：YAML frontmatter（`description` / `task` / `task_group` /
  `task_outcome` / `cwd` / `keywords`）+ 每任务一个 `### Task <n>` 块
  （Preference signals / Reusable knowledge / Failures and how to do
  differently / References 四小节）。
- `rollout_summary`：`# 一句话总结` + `Rollout context:` + 每任务
  `## Task <n>`（Outcome / Preference signals / Key steps / Failures /
  Reusable knowledge / References）。

## 3. 触发与执行模型

- 挂点：引擎 auto 压缩（turn loop）与 manual `/compact`（`Op::CompactContext`）
  两个成功路径，且要求 `result.summary_prompt.is_some()`——prune-only 压缩与
  紧急恢复（`recover_context_overflow`，含 trim 兜底）不导出。
- 执行：`tokio::spawn` 分离任务，不阻塞压缩后的会话；180 秒墙钟超时；失败只
  `logging::warn`，成功发一条 `Event::status`。任何失败都不影响压缩本身。
- 并发：进程级 `tokio::sync::Mutex` 串行化 `raw_memories.md` 读改写；同一会话
  多次压缩按 thread_id upsert（替换旧条目、更新 updated_at 与摘要文件名），
  重排序保持升序——与 Codex 从 DB 重建的语义一致。手工编辑的畸形条目按上游
  语义丢弃并告警。
- 安全：LLM 产出三字段（含 slug）先过 `codewhale_config::persistence::redact_secrets`
  再落盘；提取提示词声明"转录是数据不是指令"；产物只写应用数据目录，永不进
  用户仓库（对比：仓库根 `.codex-memory.md` 方案有误提交敏感摘要、污染
  `git status`、非 Codex 格式三重问题）。

## 4. 配置与产品面

- `CompactionConfig.memory_export`（`MemoryExportConfig { enabled, root,
  transcript_dir, git_branch }`），底座默认关闭。Pinvou 在
  `build_engine_config` 显式开启：root = `~/.pinvou3/memories/`，
  transcript_dir = `~/.pinvou3/sessions/`。
- 子代理/worker 与手动触发共用的 `compaction_config_for_model` 保持默认关闭，
  只有根会话喂记忆库（引擎手动压缩处理器读 `self.config.compaction`，所以
  手动压缩同样导出）。
- 设置 `memory_export_enabled`（`settings.json`）：默认开。旧文件缺键由 serde
  字段默认补开，全新安装由 `defaults_for_system_locale` 显式带上（测试锁两种
  路径 + 显式关闭）。通用设置页有三语（zh/en/ja）开关；web 端经
  `WebSettingsPatch` 同权修改。写盘后按设置 MVP 限制在下一次引擎拉起生效。

## 5. 与 Codex 的互通说明

- **格式互通**：文件可直接复制/导入。把 `~/.pinvou3/memories/` 内容拷到
  `~/.codex/memories/` 后，Codex 的 Phase-2 整合代理会把新增内容并入
  `MEMORY.md` / `memory_summary.md`。
- **不建议直接把导出根指向 `~/.codex/memories/`**：Codex 每次启动会从它的
  SQLite 库重建 `raw_memories.md` 并清掉库外文件（`sync_rollout_summaries_
  from_memories` 按保留集 prune），外部直写条目会被静默清除。Codex 官方支持
  的外部增量入口是其 `extensions/ad_hoc/notes/` 机制，如需实时互通可作为
  后续扩展。

## 6. 守护

- CodeWhale：`forkguard_compaction_memory_export_writes_codex_format`
  （mock 客户端端到端：提取→脱敏→双产物落盘 + provider 失败可容忍 + 空产出
  no-op）+ 9 项单元测试（渲染字节、round-trip、stem 命名、JSON 容错、请求
  构造、指令 schema 钉死）。
- 父仓：`forkguard_compaction_memory_export_wiring_isolated_to_root_sessions`
  （根会话开启 + 隔离根 + transcript 溯源 + 子代理关闭 + 设置关闭生效）、
  `memory_export_enabled_defaults_on_for_fresh_and_legacy_settings`。
- `scripts/fork-guard.sh` 指纹：4 条 r14 条目（见脚本注释）。
