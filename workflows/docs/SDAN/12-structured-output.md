# 12 · SubAgent 怎么"交活":结构化产出设计

> 2026-05-31 · 待白浪评审 → 审过再写码
> 本文用大白话写,技术细节收在每节末尾的「实现备注」里。
> 已融入 10-agent 业界调研(2025-2026)结论。

---

## 一、要解决什么问题(一句话)

**现在 SubAgent 干完活要"自觉写文件交活",但本地的 Qwen 弱模型经常不自觉——问完了不写、或者写的格式乱。我们要改成:不让它自觉,而是逼它通过一个专门的"交活窗口"提交,提交的东西不合格式就当场打回让它改,合格了由我们的代码替它写文件。**

### 现在为什么不行(实测铁证)

我们加了 transcript 日志后,亲眼看到 agent1(需求分析师)的真实过程:

1. 第 1 步:想问用户问题,但参数格式吐错了(该给数组却给了字符串)→ 被拒
2. 第 2-3 步:自己纠错,问题终于弹出卡片,用户也答了
3. 第 4 步:**拿到用户答案后,直接结束了——压根没写 brief.json**
4. 结果:brief.json 是空的 → 质检不通过 → 整个流程卡死

**病根**:让弱模型"自己拼 JSON + 自己记得写文件"是业界公认最容易崩的做法(失败率 5-10%)。10 个调研方向有 8 个独立指向同一个答案:**弱模型场景下,唯一可靠的解法就是"专门的交活工具 + 不合格当场打回"**(失败率能降到 0.1% 以下)。

> 这不是优化,是当前流程能不能跑通的命门。

---

## 二、核心思路:照搬 Claude Code 的做法

Claude Code 解决一模一样的问题,做法是(我们直接抄):

1. **给每个角色配一个专门的"交活工具",名字叫 `submit_output`。** 这个工具长什么样、要填哪些字段,由角色的"产出规格"(output_schema)决定——比如需求分析师的 submit_output 就要求填:项目标题、受众、截止日期、范围、决策机制这 5 个字段。

2. **不交活就不许结束。** 角色想结束时,如果还没成功交活,系统会拦住它,提醒"你还没提交最终结果,必须先调 submit_output"。(这步最关键——弱模型最爱"觉得差不多了就溜",必须有这道闸)

3. **交的东西不合格,当场打回让它改。** 比如缺了"受众"字段,系统不是默默接受,而是回一句"❌ 受众字段没填,请补上",让它下一轮自己修。

4. **合格了,由我们的代码替它写文件。** 角色不用自己调 write_file,它只管把内容交进 submit_output 窗口,合格后代码自动落盘成 brief.json。**这样文件格式 100% 受控,弱模型再也没机会写乱。**

**改造后,agent1 的活变简单了**:从"问完 5 问 → 自己拼 JSON → 记得写文件(还老忘)",变成"问完 5 问 → 把答案填进 submit_output(系统逼它填全)"。

---

## 三、调研补充的 5 个关键细节(比单纯抄 Claude Code 更适配弱模型)

这次 10-agent 调研发现,光抄 Claude Code 还不够——Claude Code 是给强模型设计的,我们用弱模型,有 5 个地方要专门加强:

### 1. 打回时要"指着错的地方说人话",不能只说"格式错了"

业界实测数据很惊人:同样是 Qwen,打回时只说"校验失败" vs 明确说"❌ audience 字段:期望是对象,你给的是字符串",成功率差距是 **6.75% → 99.8%**。

所以打回的话必须:① 精确指出哪个字段错了 ② 说清楚期望什么、你给了什么 ③ 用中文(别把英文报错原文丢给 Qwen,它读不懂)。最好是把它原来交的内容原样返回,在错的字段旁边标个 `// ❌ 这里要填数组`。

> 这是 60% 和 95% 成功率的分水岭,**必须做对,不能偷懒**。

### 2. 产出规格(schema)要尽量"扁平",别套娃

Qwen 对嵌套结构(尤其 anyOf/oneOf 这种"几选一"的)处理极差——实测 100% 会把嵌套字段错误地变成字符串。

所以规整 schema 时:① 顶层字段控制在 6 个以内 ② 少用"必填" ③ "让模型先想再答"的字段(比如理由)排在结论字段前面(防止它先下结论再硬凑理由)。

### 3. 加一个"完成度"字段,防止弱模型"蒙混过关"

这是调研发现我们之前**完全漏掉**的一个坑:弱模型在信息不够时,会**自己编内容把格式填满**——格式检查能过,但内容是瞎编的。

解法:每个产出规格里加一个 `completion_status` 字段,让它老实交代:`full(都问清了)/ partial(部分靠推测)/ inferred(基本靠猜)`。这样质检环节能看出"这份 brief 是真问出来的还是编的",编的就打回。

> Claude Code 没这一招,因为强模型不太会瞎编。我们弱模型必须加这道防线。

### 4. 重试次数:5 次改 3 次

Claude Code 默认让模型改 5 次。但调研一致认为:**弱模型改 2 次还不对,基本就是 schema 或提示词本身有问题,不是次数不够。** 而且 Qwen 的上下文很宝贵,反复重试会把它的"注意力"耗光。所以改成最多 3 次,3 次还不行就老实失败、报清楚原因(而不是无限耗下去)。

### 5. (可选,后做)在 vLLM 那一层就把格式焊死

最彻底的办法是让 vLLM 在"吐字"的时候就只能吐合法格式(叫 guided_json / xgrammar),这样格式错误在物理上就不可能发生,submit_output 的打回只需要兜"内容错"。

**但这个有个已知的坑**(vLLM bug #39130):当我们关了 thinking 时,这个功能可能被静默跳过。**所以这条先不做,等 submit_output 跑通后,单独验证这个坑再决定上不上。** submit_output 是应用层的、不依赖 vLLM 改动、换个模型也能用,先把它做扎实。

---

## 四、几类角色区别对待(不是所有角色都一样)

我们 10 个角色产出的东西不一样,不能一刀切:

| 角色类型 | 例子 | 怎么交活 |
|---|---|---|
| **纯结构化** | 需求分析师(brief.json)、质检(gate_report)、内容策划(outline.json) | 走 submit_output,代码落盘 |
| **一个角色产多个文件** | 素材审计(inventory+gaps 两个 json) | submit_output **交一个大对象**,代码按字段拆成多个文件 |
| **半结构化(又有结构化又有自由文本)** | 设计师(page_layout.json + base.css + DESIGN.md) | 结构化部分走 submit_output;css/md 仍自己 write_file。结束判定要兼容"既提交了 + 该写的文件也都写了" |
| **纯自由文本** | 调研(.md)、写页面(.html) | **保持现状,继续 write_file,不强上 schema** |

**为什么自由文本不强上 schema?** 调研实测:硬把自由写作塞进 JSON 框框,质量会暴跌(数学题准确率掉 27 个百分点)。.md 报告几千字塞进 JSON 字段纯属添乱。这些角色另想办法保证质量(见下)。

**为什么内容策划(大纲)反而要 schema?**(2026-06-02 修正)大纲不是"自由写作",而是**结构化数据**(章节/页序/逐页 title),且下游被**确定性脚本** `generate_ghost_deck.py` 消费——业界共识"机器/脚本消费 → JSON"(让脚本正则解析 markdown 标题=自找脆弱)。submit_output 让模型在 step 里自由规划、最后只把成稿大纲格式化成 outline.json,符合"先思考后格式化",不触发上面的质量暴跌。这也连根理顺了旧设计里 content_planner 在 subagent 内跑 shell 脚本(exec_shell 被审批拦)、outline.md/json 格式不一致那摊乱账。

### 自由文本角色怎么防"写残缺"

这些角色不走 submit_output,但有另一个弱模型坑:**写文件写到一半 token 用完了,文件残缺但系统以为成功了。** 所以给它们加个简单检查:角色说"我干完了"时,代码去看一眼它该产出的文件——存在吗?非空吗?不对就打回。(约 10 行代码)

---

## 五、关键决策(白浪 2026-05-31 已拍板)

### ✅ 决策 1:落盘路径统一到项目目录 `ppt-xxx/_state/`

(原隐患:transcript 落会话根、brief 期望项目目录,两边不一致 → 质检找不到产物,实测会卡 e2e。)
**定:** submit_output 落盘 + transcript 都统一到项目目录 `ppt-xxx/_state/`,跟质检脚本/flow_log/gate_reports 一致。submit_output 的 execute 拼项目目录绝对路径,不依赖底座给的会话根。

### ✅ 决策 2:重试次数定 3 次

(Claude Code 默认 5 是强模型预算;弱模型改 2 次还不对基本是 schema/提示词问题,且越试越耗上下文越糊涂。)
**定:** `MAX_STRUCTURED_OUTPUT_RETRIES = 3`,独立计数不占 max_steps,超 3 次 → Failed + 报清原因。**前提:必须配合"打回说人话"(第三节细节 1),否则 3 次都是盲改。**

### ✅ 决策 3:加完成度字段 completion_status

(弱模型信息不够时会自己编内容填满字段,格式过但内容假,schema 校验抓不到。)
**定:** 每个结构化角色 schema 加 `completion_status: enum[full, partial, inferred]`,角色提示词加一句"如实交代完成度,没问到的别编";质检据此过滤(如 brief 要求 full 才放行,inferred 打回重问)。
**已知局限:** 靠模型自觉填,会瞎编内容的模型也可能瞎填成 full;但"明着问成色"比"不问"能挡一部分,属低成本加层防护,不是万能。

### ✅ 决策 4:guided_json(vLLM 生成层焊死格式)缓做

(更彻底但有 vLLM bug #39130:关 thinking 时 guided_json 可能被静默跳过,需先实测。)
**定:** 先把应用层 submit_output 做扎实 + agent1 验证有效;之后单独立项实测 #39130(guided_json + 关 thinking 能否共存),再决定上不上。submit_output 不依赖 vLLM 改动、provider 无关、可移植,是可靠主路;guided_json 是后补的锦上添花。

---

## 六、改动范围 & 风险

- **底座 fork**(`tools/subagent/mod.rs`):加 submit_output 工具 + 改结束判定 + 校验打回 + 代码落盘,约 80-120 行。
- **角色配置**(`agent_registry.json`):4 个结构化角色的 schema 规整成标准格式 + 拍平 + 加 completion_status。
- **pinvou3 接线**(`harness.rs`):spawn 角色时把它的 schema 传下去。

**风险评估**:
- 改动都是"加法"——对不走 submit_output 的自由文本角色零影响,出问题只影响结构化角色。
- 符合 fork-policy §2 的提上游触发条件（所有 CodeWhale embedder 都受益，非 pinvou3 专用），做完考虑提给底座上游 PR。
- 用 agent1 验证:跑一次 → 看 transcript 里它是否调了 submit_output、打回后是否改对、brief.json 是否由代码落盘且 5 字段齐全 → 质检通过 = 成功。

---

## 七、这事在整个架构里的位置(放心,没动骨架)

调研结论:**我们的 SubAgent 架构(单 Router + 傻腿 SubAgent + 信封协议)是 2026 业界共识,10 个方向无一反对。** 这次改的只是 SubAgent"怎么交活"这一个环节,骨架完全不动。

同时调研明确警告**别做**的(我们也确实没打算做):
- ❌ 别上多 SubAgent 并行(单机 GPU 扛不住 + 弱模型协调反而更差)
- ❌ 别让 SubAgent 之间直接通信(需要强模型)
- ❌ 别让 SubAgent 再生 SubAgent(防 token 爆炸)
- ❌ 别用 Qwen 评 Qwen 当硬性关卡(它评不准)

---

## 附:相关文件(实现时要动的)

- 本文档(设计)
- `03-protocol.md` — Task 信封可能要加 output_schema / completion_status 字段
- `agent_registry.json` — 4 角色 schema 规整
- 底座 `CodeWhale/crates/tui/src/tools/subagent/mod.rs` — submit_output 主体

## 附:技术实现备注(给写码用,评审可跳过)

- 合成工具:`SubAgentToolRegistry` 里按 output_schema 动态生成 `submit_output`,input_schema=角色 schema,加在 `tools_for_model` 返回列表末尾,白名单豁免。
- 结束判定:`run_subagent` loop 在 `tool_uses.is_empty()` 处加判断——有 schema 且没提交过 → 不 break,push user 消息催它提交;重试独立计数不占 max_steps;超 3 次 → Failed。
- 校验库:优先找底座现成的 JSON Schema validator,没有就先用轻量校验(必填字段存在+类型对)起步,别一上来引重依赖。打回消息要 path + expected/got + 中文化。
- Result:`SubAgentResult` 加新字段 `structured_output: Option<Value>`(别复用 result,保持干净)。
- schema 传递:harness spawn 时从 registry 读 output_schema 塞进 assignment 传下去(不改 Op,改动面最小)。
- 落盘:submit_output 的 execute 里,按 schema 顶层 key → registry outputs 路径映射写文件;落盘根用项目 dir(决策点 1)。
- 半结构化:submit_output 额外收一个 `fs_writes: [{path}]` 清单,结束时代码 stat 校验这些文件存在+非空。
