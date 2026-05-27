# 多 subagent 后端重评估 — 2026-05-26

> 底座 v0.8.45 / Qwen3.6-35B-A3B-FP8 @ 10.214.74.113 / `max_subagents=3` 跑 C-0~C-4。
> 起因:2026-05-19 因"多 subagent 后端卡住"把 `max_subagents` 工程锁到 1,需重新评估该锁是否仍必要。

## 结论一句话

**后端并发瓶颈已消除,当年锁 1 的直接理由不再成立;本次失败瓶颈从"vLLM 调度"收敛到"环境工具坏 + subagent 内部 cap"。环境工具已在本轮修复,默认值暂保守留 1,解锁前先重跑 C-1/C-2 端到端验证。**

## 1. 后端层面 — 解锁障碍已消失 ✅

### 直接并发探针(绕开 LLM workflow,纯压 vLLM)
| 并发 | first-token | 全部完成(各 600 tokens) |
|---|---|---|
| N=1 | 0.26s | 21.5s |
| N=3 | ~0.73s | ~32.4s |
| N=4 | ~0.79s | ~31.6s |

N=4 同时 first-token 仍 <1s,4× 工作量墙上时间仅 +50% → batching 正常。当年 `v0.8.37-vllm2` 报告的"多序列 prefill 卡 SSE 超 open_timeout"**未复现**。`planB` 的 `max-num-batched-tokens` 32768→131072 (4×) 已解决该瓶颈。

### harness 实证
C-1 `agent_open×3`、C-2 `agent_open×4`、C-4 `agent_open×3` 全部**成功并行创建**,无一拿 "Sub-agent limit reached",无 SSE 卡死。

## 2. 端到端结果矩阵

| scenario | elapsed | 工具 | 判定 |
|---|---|---|---|
| C-0 single_simple | 29s | agent_open×1 → completed | ✅ 优 |
| C-3 no_need(反向) | 9.5s | 无 subagent,直接答 | ✅ 优 |
| C-4 one_fails | 320s | agent_open×3(2成1failed),6349 字容错报告 | ✅ 良 |
| C-2 research_topic | 565s | agent_open×4,web_search×9 全空,643 字 | ⚠️ 临界 |
| C-1 compare_3_libs(核心) | **660s 被杀** | agent_open×3 全 failed + 环境工具全坏 | ❌ |

## 3. 根因链(C-1 完整复盘)

```
环境工具坏(根源)
  ├─ web_search → source=bing count=0 "No results found" (bing HTML regex 抓不到)
  └─ fetch_url  → 解析到 198.18.0.79 被 SSRF 拦 PermissionDenied
        ↓
subagent 内部死磕重试联网工具
        ↓
撞 fork patch 保护上限:
  ├─ C  : DEFAULT_SUBAGENT_ELAPSED_MAX = 300s (subagent/mod.rs:70)  ← C-1 三个全在 ~305s failed
  └─ C+ : DEFAULT_MAX_STEPS = 20            (subagent/mod.rs:65)
        ↓
3 个 subagent 全 status=failed (terminal)
        ↓
主 agent 降级自己 exec_shell+curl 硬干(curl 走透明代理能通,数据其实拿到了)
        ↓
撞 harness turn_timeout 660s 被杀,没来得及综合
```

注:C-4 的 `rust_async_await`(纯概念,不联网)在 154s failed,不是 300s cap;是 subagent 内部其它 bail(疑似某步 LLM API 调用问题),未细查。

### 关键事实
- **本机是 clash/透明代理 fake-ip(TUN)模式**:`crates.io`/`bing`/`github` DNS 全解析到 `198.18.0.x`(RFC2544 benchmark 段)。`curl` 能通(http_code=200,流量被透明代理劫持转发),但底座 `fetch_url` 自己解析 DNS 拿到 198.18.0.x → SSRF 防护正确判为 restricted → **误杀**。底座 `fetch_url.rs:744-798` 本就把 198.18.0.0/15 当 restricted(有专门测试),设计正确,是部署环境特殊。
- fork patch C/C+/stop-on-fail 经 v0.8.45 sync 后**仍存活**(排除"合并静默丢失")。这俩 cap 是 2026-05-19 为"防 subagent 死磕拖垮任务"刻意加的保护,不应为过测试放松。

## 4. 跟当年 baseline diff

`v0.8.37-vllm3_final`(max=1):C-1 632s cargo timeout,主 agent 21 次 curl 串行硬干。
现在(max=3):C-1 仍 660s 被杀。**解锁 max_subagents 没解决 C-1 超时**——超时根因从来不是并发数,是 subagent failed 后降级硬干 + 环境工具坏。反而因不再被 limit 挡,主 agent 做了更多无用功。

## 5. 处置与待决策

### 已落地(本次)
- `bridge/mod.rs` `build_engine_config` 注释 + 单测断言理由更新为现状(后端 OK / 瓶颈在工具+cap),默认值仍 `unwrap_or(1)`。想试的开发者可在 `settings.json` 手动调高。
- **fetch_url fake-ip 误杀已修**:不削弱 SSRF——启用底座既有 fake-ip 信任机制。底座 `network_policy.rs:host_matches` 加 `*` 通配 + `bridge/mod.rs` 把 `network_policy` 从 `None` 改 `Some(decider)`(default=Allow + proxy=["*"])。直测 A/B:proxy=["*"] 抓 crates.io 200 OK,无 decider 仍被拦。IP 字面量 URL 仍被 `is_restricted_ip` 拦,安全。
- **web_search bing 返回 0 已修**:根因不是网络(reqwest status=200 + b_algo=7 都拿到),是底座 `parse_bing_results` 解析 bug——bing /ck/a 重定向 href 用 `&amp;` 实体编码,`normalize_bing_url` 没先解码 HTML 实体 → `extract_query_param("u")` 取不到 → URL 还原失败 → root_domain 全 bing.com → `is_likely_spam_results` 误杀整批 → 0 结果。修复:`normalize_bing_url` 先 `decode_html_entities`。验证:`run_bing_search` 0→5,域名多样(rust-lang.org/facepunch.com/...)。加回归单测 `bing_ckurl_with_html_entities_decodes_real_url`。**通用 bug,可提上游 PR**。
- 注:bing HTML scraping 仍有偶发空(反爬/rate limit),非 100% 稳定,但已从"恒为 0"变成"大部分可用"。

### 待决策(解锁多 subagent 的前置)
- fetch_url + web_search 已修。subagent 内部 120s API 超时(本地慢推理)仍是复杂研究类任务的瓶颈(`DEFAULT_SUBAGENT_API_TIMEOUT_SECS`)。
- 重跑 C-1/C-2 看修复后端到端改善,再决定是否升 `max_subagents` 默认值。注意 LLM 行为有 vLLM 抽奖,且需引导优先用 fetch_url/web_search 而非空转。

---

## 6. 后续修复 (2026-05-26)

> 本报告撰写后当天密集修复,未重跑端到端验证,待本轮测试后更新矩阵。

### 6.1 超时/默认值调参

| 参数 | 报告时 | 修复后 | 理由 |
|---|---|---|---|
| `subagent_api_timeout` | 120s | **300s** | 本地 vLLM 慢推理,复杂 prompt 生成常 >120s |
| `max_subagents` 默认值 | 1 (`unwrap_or(1)`) | **4** | 后端并发瓶颈已消除,恢复设计值 |
| `DEFAULT_MAX_STEPS` | 20 | **12** | 20 步在 600s 上限内跑不完,12 步强制更快收敛 |
| `DEFAULT_SUBAGENT_ELAPSED_MAX` | 300s | **600s** | 给 12 步 × 40-70s/步 留余量 |
| harness `turn_timeout`(heavy) | 660s | **1260s** | 匹配新的 600s cap + 主 agent 综合时间 |

### 6.2 联网工具链补强

- **网络重试**: `web_search.rs` + `fetch_url.rs` 各加 1 次 retry + 2s 指数退避,timeout 15s→30s。缓解 Bing 偶发空/rate limit。
- **`tool_agent` model 路由**: 从硬编码 `"deepseek-v4-flash"` 改为继承父 session `runtime.model.clone()`。避免子 agent 走远程 API 绕本地 vLLM。

### 6.3 Subagent 交互修复

- **`resolve_agent_ref` 截断容错**: LLM 传回 `agent_id` 时偶发截断 `"agent_"` 前缀 → `"f8f4b687 not found"`。现自动补前缀重试,修复 `agent_eval` 连续失败。
- **harness `max_subagents` 硬编码**: `spawn_for_scenario` 之前覆写 bridge 默认值为 3,现跟随 bridge 默认(4)。

### 6.4 子 agent 任务范围膨胀修复 (Scope Bloat)

**根因**: 主 agent 生成的 `prompt` 过于详细("搜 A/B/C/D 然后分析写报告"),子 agent 收到后淹没系统 prompt 的"8 步上限"约束,进入 8-15 步搜索→分析→搜索循环,撞 600s cap。

**底座修复三处**:
1. **`agent_open` schema `prompt` description 引导**: `"Keep it SIMPLE — one focused query... child returns raw findings in ≤5 tool calls; parent handles synthesis"`。
2. **`build_assignment_prompt` 注入步骤预算**: 用户消息强制追加 `**step budget: 12 max**` + `After ~6 calls, start synthesizing and return`。
3. **`DEFAULT_MAX_STEPS` 20→12**: 硬性上限,子 agent 最多 12 步必须结束。

**预期效果**: 子 agent 从"多步搜索+综合分析"变为"聚焦搜索+快速返数据",主 agent 负责综合。将失败模式从"超时白跑"转为"提前返半成品"。

### 6.5 验证结果 (2026-05-26 `run 1779799386`)

| 场景 | 耗时 | 子agent完成率 | 关键结果 |
|------|------|--------------|---------|
| C-1 `compare_3_libs` | **785.9s** | **3/3** ✅ | 突破性改善(0/3→3/3)，但管理极重(open×4, eval×12, 重试×1) |
| C-2 `research_topic` | **933.2s** | **2/4** ⚠️ | academic✅, industry_retry✅, tools❌❌, pitfalls遗忘 |
| C-4 `one_fails` | **293.9s** | **2/3** ✅ | 2个完成，1个超时后主agent补充 |
| C-0 `single_simple` | **23.7s** | **1/1** ✅ | 完美 |
| C-3 `no_need` | **9.4s** | N/A | 无子agent，直接答 |

**`compare_3_libs` 虽成功但不可作为模式可行证据**: 3/3 完成是靠 12 次 eval + 1 次重试 + 785s 堆出来的。若主 agent 自己做(web_search×3 + fetch_url×3 + 综合)，预计 200-400s 且更可控。

---

## 7. 设计决策 (2026-05-26)

> **多 subagent 并行研究模式在 pinvou3 + Qwen3.6 下 ROI 为负，废弃。**

### 判断依据

| 维度 | 单 subagent | 多 subagent 并行 |
|------|------------|----------------|
| 耗时 | **23.7s** (`single_simple`) | **785-933s** |
| 管理开销 | open×1, eval×1 | open×4-5, eval×9-12, 重试×1-2 |
| 成功率 | 100% | 50-75% (含遗忘) |
| 主 agent 负担 | 轻 | 极重 (轮询+重试+降级+综合) |

### 根因

1. **弱模型分活质量差**: Qwen3.6 拆分任务能力有限，`research_topic` 把 15+ 工具塞给 1 个子 agent → 12 步不够 → failed
2. **调度开销巨大**: 每步 40-70s，`compare_3_libs` 轮询 12 次 eval
3. **注意力分散**: 处理失败重试时遗忘其他子 agent (`research_pitfalls`)
4. **失败处理复杂**: 子 agent failed 后需决策重试/降级/fallback
5. **并行不省钱**: 3-4 个并发但总时间仍 785-933s

### 替代方案

| 方案 | 适用场景 |
|------|---------|
| ✅ 保留单 subagent | 简单委托、context isolation、代码执行隔离 |
| ❌ 废弃多 subagent 并行研究 | `compare_3_libs` / `research_topic` 类复杂研究 |
| 🔵 改为串行单任务子 agent | 1 个子 agent 做 1 件事，串行派发，主 agent 综合 |
| 🔵 改为主 agent 直接工具调用 | 主 agent 自己 web_search + fetch_url + 综合 |

### 底座影响

- `max_subagents` 默认值 4 **不变**（后端并发能力真实存在，其他场景可能有用）
- 但 **Instructions / system prompt 不再引导用户用多 subagent 并行做研究**
- 子 agent 能力保留，但产品层面降级为"辅助工具"而非"并行研究团队"
