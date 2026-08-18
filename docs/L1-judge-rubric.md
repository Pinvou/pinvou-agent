# L1 Judge Rubric — Claude 离线评 Qwen 答案质量

> **当前版本: r2** (2026-05-18 起生效)。改版规则见 §6。
>
> 用法:L1 跑完 → 用户对话框跟 Claude 说 "评一下 `target/l1-runs/<ts>`" → Claude 按本文件 rubric 评分写报告 → `target/l1-judge/<ts>-report.md`。
>
> **跟 L1 cargo test PASS/FAIL 完全解耦**。L1 是行为契约(文件落盘/耗时上限,judge 看不见的硬指标),judge 是答案质量评估,两件事。
>
> Judge 是 Claude(本对话里的我),不是远程 Anthropic API,不是本地 Qwen 自评。**跨模型独立性是 judge 的存在价值**——Qwen 跑、Claude 评,比 Qwen 自评 Qwen 多一层防同向漂移。

---

## 1. 输入 / 输出位置

```
target/l1-runs/<ts>/<scenario>.md     ← harness 自动落档 (record_transcript)
target/l1-judge/<ts>-report.md        ← Claude 评分后写这里
```

`<ts>` 是 unix epoch seconds,同一次 `cargo test --ignored` 跑下的所有 scenario 共享一个 `<ts>` 目录。

---

## 2. 评分 rubric (4 维 × 1-5 分)

### 维度 1:准确性 (Accuracy)

> "答案对不对、有没有幻觉、有没有答非所问"

| 分 | 判别 |
|---|---|
| 5 | 答案完全命中任务要求,事实/译文/代码无错误 |
| 4 | 基本正确,有微小瑕疵(标点、用词、单位) |
| 3 | 主旨对但有明显错漏(漏掉一个条件、举错例) |
| 2 | 部分错误或答非所问(理解任务一半) |
| 1 | 完全错误 / 拒答 / 严重幻觉 / 任务理解错 |

### 维度 2:完整性 (Completeness)

> "任务要求覆盖了多少"

| 分 | 判别 |
|---|---|
| 5 | 任务全部要求覆盖,该写的都写了 |
| 4 | 覆盖 80%+,漏掉的是次要点 |
| 3 | 覆盖 50-80%,漏了应该写的部分 |
| 2 | 仅触及核心,大段缺失 |
| 1 | 严重缺失,基本没回答 |

### 维度 3:简洁性 (Concision)

> "废话多不多、是不是该停就停"

| 分 | 判别 |
|---|---|
| 5 | 该说的说,该停就停,无废话 |
| 4 | 略有冗余但可接受 |
| 3 | 有 1-2 段重复或多余 |
| 2 | 啰嗦,反复说同一个意思 |
| 1 | 大量废话 / 卖弄 / 自我重复 |

### 维度 4:工具使用合理性 (Tool Usage)

> "工具选对没,次数对没,有没有过度调用或该调没调"

| 分 | 判别 |
|---|---|
| 5 | 工具选对、次数对,每次调用都有效推进任务 |
| 4 | 工具选对但次数略多/少,无副作用 |
| 3 | 1-2 次工具选择存疑(可以用更合适的) |
| 2 | 工具用乱(该调没调 / 过度调用 / 调了无关工具) |
| 1 | 完全错误的工具选择 / 应该调工具却纯文本回答 |
| N/A | 任务本身不需要工具 (例如简单翻译/问答) |

### 维度 5:任务拆分合理性 (Subagent Decomposition) — **r2 新增**

> "调 subagent 时,任务边界划得清不清楚"

仅当 scenario 涉及 subagent 工具 (`agent`) 时评分。否则 N/A。

| 分 | 判别 |
|---|---|
| 5 | subagent 边界清晰互不重叠,每个 subagent 有独立可完成的具体任务,数量合理(任务规模决定) |
| 4 | 边界基本清晰,有 1 个小重叠或多余 subagent |
| 3 | 任务拆分有缺陷(N 个 subagent 做 1 件事 / 或 1 个 subagent 塞 N 件) |
| 2 | 拆分明显错(瞎并发同样查询 / 或该拆没拆塞 1 个 subagent) |
| 1 | 不该开 subagent 却开了 / 或边界完全乱套 |
| N/A | scenario 不涉及 subagent 工具 |

### 维度 6:结果综合能力 (Result Synthesis) — **r2 新增**

> "拿到多个 subagent 返回后,主 agent 是否真的综合了 vs 简单 concatenate"

仅当 scenario 涉及 subagent 且拿到了 ≥2 个 subagent 结果时评分。

| 分 | 判别 |
|---|---|
| 5 | 主 agent 给出真正综合的结构(对比表/分类/推荐),不只是并列复述 |
| 4 | 综合到位,但有 1 处简单复述或冗余 |
| 3 | 部分综合(给了对比但没推荐 / 给了清单但没归类) |
| 2 | 几乎是 concat(N 个 subagent 的输出依次贴出来) |
| 1 | 完全没综合 / 或综合错误丢失关键信息 |
| N/A | scenario 不涉及 subagent / subagent 只 1 个 / subagent 失败没拿到结果 |

**N/A 处理**:平均分计算时跳过 N/A 项,只算实际有评分的维度数。

---

## 3. 评分操作流程 (Claude 每次跟这个步骤跑)

### Step 1: 列 transcript 目录
```bash
ls target/l1-runs/<ts>/
```
拿到所有 `*.md` 文件列表。

### Step 2: 逐个 Read transcript
对每个 `.md`,提取:
- scenario 名 + mode/phase
- user prompt
- tool timeline (注意工具调用次数和顺序)
- assistant final text
- tool_call_histogram + elapsed (meta 块)

### Step 3: 按 rubric 评每个维度

**关键评判原则**:
- **准确性看 final text**——文本是否答对任务、有没有幻觉
- **完整性看 prompt 要求 vs text 覆盖度**——prompt 列了几条要求、text 回应了几条
- **简洁性看 text 长度 vs 必要信息量**——可参考 text_chars meta 字段做参考但不绝对
- **工具使用合理性看 timeline**:
  - prompt 明确说"用 write_file"→ 应该调 write_file
  - 一 turn 内连续多个相同工具→检查是否合理(`batch_create_7_files` 期望 7 次 write_file)
  - Plan 模式 → 不应调 write/edit/exec_shell(底座 sandbox 应该已拦,但 LLM 不该尝试)
  - "不要先 list_dir 探目录" → 不应调 list_dir
  - **白名单外工具出现** → 默认工具使用 ≤2 分（`rlm`/`tasks`/`automation`/`github`/`Run` 等未放出家族，以及 `git_*`/`apply_patch`/`exec_shell*` 等 v0.9.5 已退役旧名——模型可见面以 `pinvou3-app/src-tauri/src/features/assistant/tool_policy.rs` 的 `PINVOU3_ALLOWED_TOOLS` 白名单为准，完整机制见 `docs/capability-governance.md`）
  - **过激探目录** → 工具使用 ≤3 分 (translate/简单 QA 等任务调 list_dir/read_file 探环境)
- **任务拆分合理性 / 结果综合能力 (r2 维度 5/6)**:仅 subagent scenario 评。看 `agent` 工具调用的任务描述、subagent 数量、拿到 subagent 结果后主 agent 输出综合度

**每个分附一句话理由**——理由必须具体引用 transcript 内容,不能空洞("还可以"不算理由)。

### Step 4: 写报告到 `target/l1-judge/<ts>-r<N>-report.md`

`<N>` 是当前 rubric 版本号(本文档顶部"当前版本"),比如 `1779074272-r1-report.md`。
`mkdir -p target/l1-judge/` 若不存在。按 §4 模板。

### Step 5: append 离群点到 `process.md` (闭环防丢失)

任一 scenario 任一维度 ≤3 → 把改进建议 append 到 `process.md` 末尾的固定区:

```markdown
## L1 judge 离群点跟进 (auto-append by Claude)

### <date> · run <ts>-r<N> · <scenario> · <维度> <分>/5
- **问题**: <从 judge 详评里抽出来的具体描述>
- **改进方向**: <你给的具体建议,例如改 reminder/instructions/prompt 哪段>
- **状态**: 🆕 待处理
```

`<date>` 用今天日期(用户的 currentDate),`<ts>-r<N>` 跟 report 文件名一致。

如果 `process.md` 末尾没有 `## L1 judge 离群点跟进` 这个 H2,先 append 一个。
如果同一 scenario 同一维度的待办已存在(grep 同 scenario+维度),不重复 append,改更新状态行为 "🔁 又出现一次"。

### Step 6: 给用户简短回报

对话框里告诉用户:
- 总平均分
- 离群点条数 (写进 process.md 第 N 行)
- 报告路径

不要在对话框贴完整报告——报告在文件里,用户自己看。

---

## 4. 报告模板

````markdown
# L1 Judge Report — `<ts>` (<scenario_count> scenarios, rubric r<N>)

> Judged by Claude (本对话). Rubric: `docs/L1-judge-rubric.md` **r<N>**.
> Source transcripts: `target/l1-runs/<ts>/`.

## 总览

| scenario | 准确性 | 完整性 | 简洁性 | 工具 | 拆分 | 综合 | 平均 |
|---|---|---|---|---|---|---|---|
| translate_no_tool | 5 | 4 | 4 | N/A | N/A | N/A | 4.33 |
| batch_create_7_files | 5 | 5 | 4 | 5 | N/A | N/A | 4.75 |
| subagent_compare_3_libs | 5 | 5 | 4 | 4 | 5 | 4 | 4.50 |
| **维度平均** | ... | ... | ... | ... | ... | ... | **N.NN** |

(拆分=任务拆分合理性,综合=结果综合能力。两列只对 subagent scenario 评。)

## 逐 scenario 详评

### translate_no_tool — 4.33

- **准确性 5/5**: 译文 "We are testing a locally deployed AI assistant." 准确,无歧义
- **完整性 4/5**: 译文完整,但加了句末多余感叹号,prompt 没要求情感色彩
- **简洁性 4/5**: 直接给译文,但前置加了 "Translation:" 标签 prompt 未要求
- **工具使用 N/A**: 任务本身不需要工具

### batch_create_7_files — 4.75
...(每 scenario 4 项 + 一句话理由)

## 离群点

### ⚠️ 需关注 (任一维度 ≤2 或平均 ≤3.0)

- 无 / 或列出 scenario + 原因

### ✅ 全优 (全维度 ≥4.5)

- save_to_tmp_no_validate_fail

## 跟历史 baseline diff (可选)

如果 `target/l1-runs/` 下有更早 ts,跟最近一次对比:

| 维度 | 上次 (`<old_ts>`) | 本次 (`<ts>`) | Δ |
|---|---|---|---|
| 准确性 | 4.4 | 4.6 | +0.2 |
| 完整性 | 4.5 | 4.4 | -0.1 |
| 简洁性 | 4.0 | 4.2 | +0.2 |
| 工具使用 | 4.5 | 4.67 | +0.17 |
| **总平均** | 4.35 | 4.43 | +0.08 |

**重大变化** (任一维度 ±0.5+):
- 无 / 或列出维度 + 可能原因

## process.md 待办建议 (闭环)

任一维度 ≤3 → 这里列出,**同时**已 append 到 `process.md` 末尾的 `## L1 judge 离群点跟进` 区:

- `<scenario>` <维度> <分>/5 → 建议: <具体改进>

## 备注

- Judge 自评的固有偏差: Claude 跟 Qwen 都是 LLM,虽跨模型但同类心智,某些"模型味"可能 Claude 看不出来
- 这是 ad-hoc judge,不是 CI gate。要 release 前手动跑 + 看报告
````

---

## 5. 历史 diff 玩法

跑两次 L1 → 拿两个 `<ts>` 目录 → Claude 用同一个 rubric 评两次 → 报告里互相对照。

典型用法:
- 改了 INSTRUCTIONS_MD → 跑前/跑后 diff 看质量变化
- 升级 vLLM / Qwen 模型 → diff 看新模型有没有掉链子
- 改了 system_prompt / reminder → diff 看引导效果

**diff 不是绝对**:同 prompt 同模型多次跑也会有 ±0.2 的波动(LLM 本质不确定),所以 ±0.5 才算 signal。

---

## 6. Rubric 演进 & 跨版本 diff 规则

### 版本历史

- **r1** (2026-05-18): 初版,4 维 × 1-5 分,N/A 跳过。Step 3 增加 blocklist 工具/过激探目录扣分条款。
- **r2** (2026-05-18): 加 2 维 (任务拆分合理性 / 结果综合能力),用于 subagent scenario。

### 改版流程

1. **改 rubric 内容**(新增维度/调整判别标准/扣分条款)→ 顶部 "当前版本" bump 到下一个 r
2. **§6 版本历史**加一行说明改了什么
3. **所有未来 baseline 文件夹命名**含新 rubric 版本: `docs/l1-baselines/v<app_ver>-r<N>/`
4. **跨 rubric 版本 diff 拒绝**——4 分(r1)跟 4 分(r2)不是一个尺子。要 diff 必须先用同一 rubric 重评一边

### 何时 bump 版本

- 加新维度(从 4 维 → 5 维)
- 改 1-5 分的判别标准(扣分门槛/示例变化)
- 加新扣分条款(像 r1 的 blocklist 工具 ≤2 分)
- **不需要 bump**:错别字、补充说明、报告模板调整

### Baseline 命名约定

```
docs/l1-baselines/v<app_ver>-r<rubric_ver>/
   ├── <scenario>.md × N
   └── judge-report.md
```

例:`v0.8.37-r1/` 表示 pinvou3-app v0.8.37 + rubric r1 评分。

升 rubric 时:旧 baseline 文件夹**不动**(它们是历史快照,用旧 rubric)。新跑用新 rubric,新 baseline 起新文件夹。需要时跑一次"用新 rubric 重评旧 transcript"产 `v0.8.37-r2/` 作为 rubric 迁移参照。

### 后续可能加的维度 (留作 r2+ 候选)

- "上下文连贯性"(multi-turn scenario 大量引入后)
- "安全性"(模型有没有泄露敏感信息 / 越权操作)
- "中文表达地道度"(zh-Hans 用户体验维度)
