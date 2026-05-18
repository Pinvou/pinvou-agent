# L1 Judge Rubric — Claude 离线评 Qwen 答案质量

> 用法:L1 跑完 → 用户对话框跟 Claude 说 "评一下 `target/l1-runs/<ts>`" → Claude 按本文件 rubric 评分写报告 → `target/l1-judge/<ts>-report.md`。
>
> **跟 L1 cargo test PASS/FAIL 完全解耦**。L1 是行为契约(工具/落盘/耗时),judge 是答案质量评估,两件事。
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

**每个分附一句话理由**——理由必须具体引用 transcript 内容,不能空洞("还可以"不算理由)。

### Step 4: 写报告到 `target/l1-judge/<ts>-report.md`

按下方模板。`mkdir -p target/l1-judge/` 若不存在。

### Step 5: 给用户简短回报

对话框里告诉用户:
- 总平均分
- 离群点(任一维度 ≤2 分,或全维度 ≥4.5 分)
- 报告路径

不要在对话框贴完整报告——报告在文件里,用户自己看。

---

## 4. 报告模板

````markdown
# L1 Judge Report — `<ts>` (<scenario_count> scenarios)

> Judged by Claude (本对话). Rubric: `docs/L1-judge-rubric.md` v1.
> Source transcripts: `target/l1-runs/<ts>/`.

## 总览

| scenario | 准确性 | 完整性 | 简洁性 | 工具 | 平均 |
|---|---|---|---|---|---|
| translate_no_tool | 5 | 4 | 4 | N/A | 4.33 |
| batch_create_7_files | 5 | 5 | 4 | 5 | 4.75 |
| plan_mode_list_dir | 4 | 4 | 3 | 4 | 3.75 |
| save_to_tmp_no_validate_fail | 5 | 5 | 5 | 5 | 5.00 |
| reasoning_off_speed | 4 | 4 | 5 | N/A | 4.33 |
| **维度平均** | 4.6 | 4.4 | 4.2 | 4.67 | **4.43** |

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

## 备注

- Judge 自评的固有偏差: Claude 跟 Qwen 都是 LLM,虽跨模型但同类心智,某些"模型味"可能 Claude 看不出来
- 这是 ad-hoc judge,不是 CI gate。要 release 前手动跑 + 看报告
- Rubric 改了请 bump rubric 版本号
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

## 6. Rubric 演进

v1 (2026-05-18): 初版,4 维 × 1-5 分,N/A 跳过。

后续可能新增:
- "上下文连贯性"(multi-turn scenario 引入后)
- "安全性"(模型有没有泄露敏感信息 / 越权操作)
- "中文表达地道度"(zh-Hans 用户体验维度)

每次改 rubric 都要 bump 版本号,旧报告标注用的版本以便 diff。
