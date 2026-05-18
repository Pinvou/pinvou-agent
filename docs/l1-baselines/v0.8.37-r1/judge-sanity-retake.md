# Judge 自洽 Sanity Retake — v0.8.37-r1

> 流程目的: 验证 Claude judge 是否稳定按同一 rubric 评分。
> 操作: 重读 5 个 transcript,**不偷看** 原 `judge-report.md`,按 rubric r1 重评。
> 跟原报告 diff 看漂移幅度。

## 重评结果 vs 原报告 diff

| scenario | 准确性 | 完整性 | 简洁性 | 工具 | 平均 |
|---|---|---|---|---|---|
| translate_no_tool | 5→5 (0) | 5→5 (0) | 5→5 (0) | N/A | 5.00→5.00 (0) |
| batch_create_7_files | 5→5 (0) | 5→5 (0) | 4→4 (0) | 5→5 (0) | 4.75→4.75 (0) |
| plan_mode_list_dir | 4→4 (0) | 4→4 (0) | 3→3 (0) | 5→5 (0) | 4.00→4.00 (0) |
| save_to_tmp_no_validate_fail | 5→5 (0) | 5→5 (0) | 5→5 (0) | 5→5 (0) | 5.00→5.00 (0) |
| reasoning_off_speed | 5→5 (0) | 5→5 (0) | 5→5 (0) | N/A | 5.00→5.00 (0) |
| **总均分** | 4.8 | 4.8 | 4.4 | 5.0 | **4.75→4.75 (0)** |

**所有 5 scenario × 4 维度 = 20 评分点,0 漂移**。

## 但这个结果不能尽信 ⚠️

**Sanity 的真正问题**: 我（Claude）就是写原报告的人——同一 session 同一 context,记忆里有锚定。重评的"独立性"打折严重:

1. **记忆污染**: 写原报告的过程把分数烙进我的 working memory
2. **transcript 明确性**: 5 个 scenario 都"明确成败"(write_file 成 7 个就 5 分,没成 1-2 分),争议空间小
3. **无模糊地带**: 没有需要主观判断的 scenario (像"plan 写得好不好"这种 4 vs 5 分模糊带)

## 真正的 sanity 该怎么做

| 方法 | 独立性 | 成本 | 何时做 |
|---|---|---|---|
| 同 session 重评 (本次) | ❌ 低 | 0 | 验证流程而已 |
| **新 session Claude 重评** | ✅ 中 | 1 次对话 | 每个新 baseline 锚定后做 |
| **几个月后 Claude 重评** | ✅ 高 | 等时间 | 长期项目漂移检测 |
| **另一个模型 (GPT/Gemini) 评同 rubric** | ✅✅ 最高 | API 钱 | 重大里程碑前 |

## 结论

- **流程通了**: rubric → transcript → Claude 评 → 报告 → process.md 闭环
- **本次数据不算 sanity**: 同 session 重评是 sanity "演习" 不是 "考试"
- **真 sanity 留给后续**: 等下一次锚 baseline 时,**开新 session 让 Claude 不带记忆评一次**——那才是有效信号
- **如果发现真漂移** (>±0.5): rubric 判别标准不够明确,需 r1 → r2 加更具体的扣分示例

## 跟用户的协议

下次锚 baseline 时,你 (用户) 提醒我"开新 session 重评"——我故意开新 conversation 评同一批 transcript,产 `judge-sanity-retake-newsession.md`。那才是真 sanity。
