# Eval 产品评分与诊断摘要设计

## 目标

在现有 Product smoke 的 JSONL、确定性规则、独立 Judge 和 Markdown 报告之上，增加两项面向产品决策的能力：

1. 稳定、可解释、可跨内部版本比较的 `Pinvou Product Score`；
2. 将底层 finding 归纳为“产品存在什么问题、先改什么、如何验收”的产品问题摘要。

本设计不把私有 Product smoke 分数包装成公开榜单成绩。与 BFCL 等网上榜单横向比较必须走未来独立的 `official-compatible` adapter。

## 评分双轨

### 轨道 A：Pinvou Product Score

Product Score 为确定性 `0..100` 整数，由本次运行的结构化记录和规则 finding 计算。Judge 不参与该总分，保证 Judge 未配置、超时或输出无效时，同一批运行数据仍得到相同分数。

报告固定展示五个子分：

| 子分 | 权重 | 主要依据 |
|---|---:|---|
| 任务完成 | 35 | Completed 比例、case failure、timeout |
| 工具可靠性 | 25 | 必需工具缺失、工具执行失败、重复调用 |
| 约束遵循 | 15 | 禁止工具却调用、状态与预期不一致 |
| 性能效率 | 15 | 延迟离群、高 token 慢请求 |
| 运行稳定性 | 10 | cache 命中、runner/provider 类失败 |

每个子分从 100 开始，按确定性 finding 和失败记录扣分，最低为 0。总分为五个子分的加权和，四舍五入为整数。相同 finding 去重后只扣一次；同一 case 的不同问题可以分别扣分。

分数必须同时输出：

- 总分与等级：`优秀 90–100`、`良好 75–89`、`需改进 60–74`、`高风险 0–59`；
- 五项子分；
- 每条扣分项的 finding ID、case、扣分值和安全证据；
- 样本数不足 10 时的低置信提示；
- “仅用于相同 case 集、配置、模型和版本之间的内部比较”声明。

建议的确定性扣分表：

| Finding / 状态 | 子分 | 每次扣分 |
|---|---|---:|
| case failed / timeout | 任务完成 | 35 |
| tool event failed | 工具可靠性 | 30 |
| required tool missing | 工具可靠性 | 25 |
| repeated tool use | 工具可靠性 | 10 |
| forbidden / unexpected tool use | 约束遵循 | 25 |
| slow high token | 性能效率 | 20 |
| latency outlier | 性能效率 | 12 |
| low cache hit ratio | 运行稳定性 | 15 |

未知 finding 不参与扣分，但仍在报告原有 finding 区域展示，避免未来新增规则被错误计分。

### 轨道 B：公开 Benchmark Score

当前 Product smoke 报告只展示 `公开榜单分数：不可用`，并说明原因：未使用官方数据集、官方推理协议、官方评分器和固定版本。

未来 `official-compatible` adapter 应独立实现：

- 固定 BFCL 数据集与代码/评测包版本；
- 使用官方类别和评分器；
- 保留类别分数、Overall Accuracy、成本、延迟与运行元数据；
- 标注完整集或子集；子集成绩不得称为官方榜单成绩；
- 不将 Product Score 与 BFCL Accuracy 合并或换算。

## 产品问题摘要

Markdown 在“运行结论”之后新增 `## 产品问题与改进方向`，位于关键指标之前。摘要只消费已经通过安全收口的结构化 finding，不读取原始 prompt、回答、工具输入输出或错误详情。

每个产品问题使用固定字段：

- `问题领域`：任务完成、工具链、约束控制、性能、缓存/稳定性；
- `结论`：面向产品的中文归纳；
- `优先级`：P0/P1/P2；
- `影响范围`：受影响 case 数和安全 case ID；
- `证据`：结构化计数、耗时、工具名等安全事实；
- `建议动作`：明确到可修改的产品层；
- `验收标准`：下一轮 eval 可自动验证的目标。

确定性映射示例：

| Finding | 产品结论 | 建议动作 | 验收标准 |
|---|---|---|---|
| tool event failed | 工具执行链路存在可靠性问题 | 检查工具错误处理、重试和降级 | 连续 3 次同套件无工具失败 |
| required tool missing | 模型未正确选择必需工具 | 加强工具描述、路由和约束 | 对应用例必需工具调用率 100% |
| unexpected tool use | 工具权限/调度约束未生效 | 在 dispatch 前阻止禁止调用 | 禁止工具用例调用数为 0 |
| repeated tool use | 工具循环缺少停止条件 | 增加去重和循环上限 | 相同工具每 case 不超过规则阈值 |
| latency outlier | 部分产品路径存在明显长尾 | 分解模型与工具耗时并优化慢路径 | 连续 3 次不超过同批中位数 2 倍 |
| slow high token | 上下文或提示体积导致成本与延迟升高 | 缩减无效上下文、检查缓存 | token 与耗时均回到阈值内 |
| low cache hit ratio | Prompt/cache 复用不足 | 检查稳定前缀和缓存边界 | cache hit ratio 不低于 25% |
| case failure / timeout | 核心任务链路不可用 | 优先修复失败阶段和超时边界 | 对应用例连续 3 次 Completed |

同类 finding 按产品领域聚合，避免同一建议在多个章节机械重复。摘要最多展示 5 个领域，按最高严重度、受影响 case 数、稳定 ID 排序。

若没有 finding，仍输出明确结论：“本次 smoke 未发现规则可识别的问题；样本较小，不能证明产品无问题”，并给出扩大样本和连续运行建议。

## Judge 分数

Judge 成功时继续展示现有六维分数，但标题明确为 `独立 Judge 质量评分`，与 Product Score 并列而不混算。Judge 失败时展示降级状态和建议：检查 Judge 模型配置/响应格式，Product Score 仍有效。

Judge finding 可以进入产品问题摘要，但必须保持 `[AI 推断]` 标签；不得覆盖相同证据的规则事实，也不得影响确定性 Product Score。

## 报告结构

Markdown 调整为九个固定二级章节：

1. 运行结论
2. 产品问题与改进方向
3. 产品健康评分
4. 关键指标
5. 逐用例诊断
6. 工具与性能观察
7. 确定性规则发现
8. 独立 Judge 质量评分
9. P0/P1/P2 改进建议
10. 评测限制与可比性说明

其中“产品健康评分”包含 Product Score、子分、扣分解释和公开榜单不可用状态。虽然章节总数由原 8 节扩展为 10 节，既有内容和顺序关系保持，新增信息前置以便决策者快速阅读。

## 数据与隐私边界

- Product Score 与摘要只读取 `EvalRecord` 的持久安全字段和安全 finding；
- 不读取或持久化 `EvalRecord.analysis`、原始 prompt、完整回答、raw error、Judge raw response、工具输入输出；
- 所有动态文字继续经过现有 Markdown escaping、credential guard 和 300 字符安全收口；
- case ID、工具名继续使用现有 canonical/safety 规则；
- 评分明细进入 Markdown；JSONL complete 只增加可选、向后兼容的总分与版本字段，不写诊断正文。

## 版本与可重复性

评分公式标记为 `pinvou-product-score/v1`。报告记录公式版本。不同评分公式版本不得直接比较；同一公式版本也只有在 case 集、模型、产品配置和运行环境一致时才可做趋势比较。

建议趋势结论至少使用 3 次运行的中位数；单次 5-case smoke 只用于发现明显回归。

## 错误处理

- 评分输入为空时总分显示不可用，不伪造 100 分；
- 未知 finding 不扣分；
- 算术使用有界整数，避免溢出；
- Judge 不可用不影响 Product Score；
- Markdown 安全检查或原子写入失败仍沿用现有失败语义：返回非零，并保留已完成 JSONL。

## 测试与验收

至少覆盖：

- 全部成功且无 finding 时为 100，且带小样本限制；
- 工具失败、禁止工具、延迟离群分别只扣对应子分；
- 重复 finding 去重计分；
- 多 case 同类问题聚合为一个产品问题；
- 问题摘要包含建议动作和可自动验证的验收标准；
- 未知 finding 不扣分；
- 空记录为不可用而非 100；
- Judge 成功/失败均不改变 Product Score；
- Markdown 不包含原始 prompt、回答、raw error 或凭据；
- 真实 Product smoke 生成带总分、子分、产品问题摘要和可比性声明的报告。

## 非目标

- 本阶段不实现 BFCL 官方 adapter；
- 不抓取或复制网上榜单数据进入本地报告；
- 不用 Judge 分数修正确定性 Product Score；
- 不根据单次 smoke 宣称产品优于其他模型或产品。
