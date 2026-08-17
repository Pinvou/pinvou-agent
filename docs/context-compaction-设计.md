# 上下文压缩设计（总纲 + 分期计划）

> 立档 2026-07-02。本文已取代早期 256K 调参方案；旧方案对应的 fork patch 已被上游
> v0.8.65 吸收进 `route_budget`/`RouteLimits`，不再需要 fork。历史推导可从 Git 记录追溯。

## 0. 设计目标（钉死，优先于一切参数）

**pinvou 压缩的一等目标是「最小化压缩次数」，不是「省 token」。**

因为对本地 Qwen3.6 + vLLM 部署，压缩一次的真实代价不是 token，而是：

1. 一次 LLM 摘要请求（本地 vLLM 慢）；
2. **压完后整个 prefix 变了** → 下一请求冷 prefill；历史上在 MTP 开启时曾伴随工具调用
   漂移。2026-07-14 的 A/B 已确认优先关闭 MTP、保留 prefix caching，原始实测记录可从
   Git 历史追溯。

云端大窗口用户不在乎这个；pinvou 在乎。**据此，评判任何压缩改动的唯一标准是：
它让压缩次数变多还是变少。** T 在保证低于 E 的前提下尽量高；评估 seam 等重机制时
用「省多少次 prefix 重写」而非「省多少 token」衡量。

## 1. 术语与水位线

一根轴，四个量。W 是探测输入，O 派生，T/E 是两条触发线，Wall 是物理墙。

```
0 ─── T 正常压缩线 ─── E 紧急压缩线 ─── Wall 物理墙
      (app 推导)         (底座推导)        (探测所得)
      LLM 摘要主路径      强制救场安全网     provider 400
```

| 量 | 谁算 | 含义 | 公式 |
|---|---|---|---|
| **W** 窗口 | app 探测 | vLLM `max_model_len` | probe → `RouteLimits.context_tokens`；失败 → 名字 hint → 128,000 兜底 |
| **O** 输出预留 | app 派生 | 单次回复留的空间 | `clamp(业务需求 24,576, 下限 ~6,144, 上界 W/4)` |
| **E** 紧急线 | 底座，**不要自己算** | 全量输入硬天花板 | `W − O − 1,024`（turn_loop.rs:288 → context.rs:600） |
| **T** 正常线 | app 推导后填 `token_threshold` | 温和摘要触发点 | 见 §3 |

底座的 40%/75%/90% pressure 分级只用于显示，不是触发线。

### 两把尺问题（所有复杂度的根）

- **T 的判据**（`should_compact`, compaction.rs:694）：可摘要**子集**的 **raw 尺**（bytes÷4，无放大）。
- **E 的判据**（`estimated_input_tokens`, compaction.rs:658）：**全量** input 的 **conservative 尺**（raw×1.5 + system÷3 + framing）。

两条线不是一把尺，差异是**乘性的 ×1.5**，不是加性的。这就是为什么 T 必须做尺换算、
为什么阶段一 T 只能到 ~50% 而不是 75%。**真正的解法是消灭两把尺（§4 上游线），
不是把换算做得更精。**

## 2. 现状与根因

现状：`token_threshold` 写死 190,000（bridge/mod.rs:629），窗口靠 `qwen36_35b_256k`
名字后缀 hint 派生。

客户机（cube-f840）翻车根因：vLLM 启动丢了 `--served-model-name qwen36_35b_256k`，
served name 变 `qwen3.6-35b` 无 `_Nk` 后缀 → 底座兜底 128,000 窗口（实际
`max_model_len=262144` 完好）→ E 降到 105,472，而写死的 190,000 > E → **倒置**，
正常线永远轮不到，只剩 emergency 抖动（每次本地 trim 省 2-3K）。

**单点失败：窗口来源依赖一个易丢的启动参数。** 修法 = 不靠名字，直接探测。

> ✅ **已实证坐实**（2026-07-02，见 §6）：写死 190K 在**健康 256K 机器上也是倒置的**。
> 触发顺序实证（真实 `should_compact`）：262K 窗口下 emergency 在 N=198 条触发，而
> `should_compact(T=190K)` 要到 N=255 条才触发——**紧急线先，正常线永远轮不到**。之所以
> 长期没暴露，是 emergency 路径**也先跑 LLM 摘要**（engine.rs:2942），成功时体验与正常
> 路径几乎一样；直到窗口被误判成 128K，emergency 摘要救不回落到本地 trim，才以抖动暴露。

## 3. 方案总纲

### 部署档案 + 探测透传（v0.9 当前方案）

1. `monitor.rs`：`probe_vllm_served_model` → `probe_vllm_model_info`，返回
   `(Option<String>, Option<u32>)`（name, max_model_len）。`parse_models_response`
   已解析两者，现在只是别丢 max——纯透传，零新增请求。
2. `engine_pool.rs`：探测结果两个都用，name 覆盖 model（现状不变）+ 存 `max_model_len`。
3. `SavedModel`：持久化具体部署的 `context_window_tokens` / `max_output_tokens`，与发给服务端
   的 wire model alias 解耦；设置页可为任意 OpenAI-compatible 引擎配置。
4. `bridge/mod.rs`：声明窗口与 probe 取较小值，生成同时包含 context/output 的
   `active_route_limits`；默认本地 Qwen 为 262144/24576，未知 vLLM alias 回退
   128000/24576，其他引擎没有声明时不猜。
5. v0.9 底座只补最小 embed API：把宿主 limits 附到 resolved route；显式 route output
   优先于未知模型名的 4K 兼容 fallback，未声明 route 的上游行为不变。

连带收益（现状白丢的）：系统提示词窗口声明跟真值走（engine.rs:933）、请求输出 cap
按小窗自动钳制（context.rs:62）、context report 百分比 route-aware（context_report.rs:179）。

### T 推导公式（阶段一，同尺换算 + 守护余量）

```
T = (E − S − framing) / k − R − M      // 换算到 raw 子集尺，并留 margin
T = clamp(T, floor=4,096, 上界=0.75·W)
工程简化等价： T = (E − S) / 1.5 − 22,000
```

| 常数 | 含义 | 取值（**§6 实测校准 2026-07-02**） |
|---|---|---|
| k | conservative 放大系数 | **1.5（实测精确**，三档 k_eff=1.500，pinvou 关 thinking 无偏移） |
| S | system 保守估算预算 | **~4,000**（dump 实测 base ~1,405 + 运行时 workspace/memory 余量，保守取 4K） |
| framing | 每消息 12 tok + 48 | **~2,500**（随会话长度，N≈200 时 2,424；量级小） |
| R | pinned（近 4 条 + query）raw | **~4,500（实测 4,358，与会话总长无关**，恒定） |
| M | T/E 之间的安全 margin | ~15,000（conservative 尺） |

> 原文档假设 S≈12K、R≈15K 均**高估约 3 倍**；实测 S/R 都小得多，故 T 能设得更高
> （压缩更少，符合 §0 目标）。合并固定扣减 ≈ framing+R+M ≈ 22,000。

**结构定死、常数可校准**：只有 k/S/framing/R/M 五个常数要调，其余全是 W 的函数，自动
缩放，不需要「档位」。四档只是公式在代表点的**采样展示**（实测值），不写进代码：

| W | E | T（实测公式） | T/W | 触发顺序实证 |
|---|---|---|---|---|
| 262,144 | 236,544 | **~133K** | 51% | T=130K：nice 在 N=174 先于 emergency N=198 ✅ |
| 131,072 | 105,472 | **~46K** | 35% | T=45K：nice 在 N=60 先于 emergency N=84 ✅ |
| 65,536 | 48,128 | **~7K** | 11% | 贴近 floor，正常线仍先（压缩频繁） |
| < ~20,000 | 很小 | floor 4,096 | — | prompt 吃光窗口，floor 防风暴 + UI 告警 |

小窗口 T 贴 floor 是**数学结论不是设计旋钮**——固定开销在小窗口占大头。C/D 档不花精力
调参，只保证「floor 防压缩风暴 + 设置页/chip 告警不崩」。

### O 来源（v0.9）

O 是具体 route 的单轮输出上限，不再从 wire model 名字猜。默认本地档案为 24,576；用户可
按部署显式配置，最终取“档案值、Pinvou 进程级请求上限、窗口可容纳值”的较小值。底座只在
route 未声明 O 时才使用静态模型目录/未知模型 4K fallback。

## 4. 分期计划

三块，两期在关键路径、一条上游线并行。

### v0.8 一期：后端探测 + 多窗口适配（历史上零 fork）

> **实施状态（2026-07-02，分支 `pinvou3-compact`）**：✅ 代码完成 + 编译通过 + 275
> lib 测试全绿 + fork-guard `--fast` 零变化（证实零 fork）。
> ✅ **真机探测已验证**：对测试 vLLM 端点跑 `live_probe_returns_window`,
> probe→`(qwen36_35b_256k, 262144)`,端到端 derive→T=133029(非写死 190K)。
> ✅ **云端边界修复**：≥500K 窗口(云端 deepseek-v4-pro 1M 等)的 output 预留镜像底座
> `TURN_MAX_OUTPUT_TOKENS=262144`,否则 E 偏大→T 偏大→倒置(`compaction_cloud_large_window_models` 锁)。
> ✅ **小窗口告警落地**：本地推理引擎(`target_kind=local`)且探测窗口 < 128k(131072)时,前端
> 监控卡「上下文长度」行下显示琥珀色告警(三语);云端 /remote 不触发。纯前端零 fork。
> 剩余:GUI 长会话人工观察 banner（需桌面环境）+ ops 补 served-name（下方，113 机）——待用户。
> 注：O 按窗口分档(≥500K→262144,否则 max_output_tokens);云端拿不到真实窗口的模型
> (gpt-4o/qwen-max 等)退 128k 兜底(与底座同源不倒置),准确值靠二期手动配置。

> **v0.9 更新（2026-07-17）**：上面的“零 fork”是当时基线的历史结论。v0.9 resolved
> route 会对未知 wire alias 采用 4K output 兼容 fallback，因此现方案在 T6 增加了最小
> limits 宿主入口与显式 output 优先级；模型档案、探测和 Compact 推导仍全部在 app。

范围：部署档案/探测/T 推导都在 app；底座仅保留 T6 的 route limits 入口和预算优先级。

- 改：`monitor.rs`（探测透传）、`engine_pool.rs`（存 W）、`bridge/prefs.rs`
  （部署档案）、`bridge/mod.rs`（同源 route limits + T 推导）。
- ops：bundle 启动脚本补 `--served-model-name qwen36_35b_256k`（防御 + 监控标签；
  113 机 `~/workspace/pinvou3-bundle`，不在本仓则记 ops 项，不阻塞 PR）。
- 依赖：app Cargo.toml 加 `codewhale-config`（facade 不 re-export `RouteLimits`；
  已在依赖图，lock 几乎不动，别为一行 re-export 动 fork）。

**验收**：
1. `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml -- --test-threads=1`
   （bridge 测试 env 竞争，必须单线程）。
2. 守护测试参数化四窗口，断言**同尺**不变式 `k·(T+R) + S + M ≤ E`（旧的
   `compaction_threshold_stays_below_emergency_budget` 锁的是跨尺假不变式，一并改）；
   另断言 `probed context_tokens=Some` 时 `active_route_limits.context_tokens` 被填。
3. `./scripts/fork-guard.sh` 必须覆盖底座 route limits/output precedence 与 app 的
   128K/256K、自定义兼容引擎结果式回归。
4. 真机冒烟：连本地 vLLM，footer 百分比按 262,144 计；长会话 ~T 处出现
   "Auto context compaction"（正常路径）而非 "Emergency"。
5. 回归客户场景：mock probe 返回 131,072，确认正常压缩先于紧急、无抖动。

### 手动配置 context/output（已完成）

范围：设置页按 SavedModel 指定窗口与 output。声明值是部署能力上限；probe 若得到更小窗口
则向下收紧，避免配置高于真实引擎。这样同一模型在不同 endpoint 上可以有不同档案，也不会
把 vLLM/Qwen 规则误套到 llama.cpp、SGLang 或其他 OpenAI-compatible 引擎。
- 需处理：改值后是否需重启 engine（走已有「需重启」操作条约定，见 memory
  `headless_ui_smoke_check`）；非法值（< prompt 开销）拒绝 + 告警。

### 上游线（并行，不阻塞一/二期）：usage 回灌 + 同尺判据

**这是真正的「更优做法」，也是把 T 从 ~50% 提到 ~75% 的唯一途径**，但要动底座 +
上游 PR，属通用优化（所有本地/自托管小窗口用户受益，符合 fork-policy §2「通用优化才提上游」）。

- 事实基础：真实 `usage.input_tokens` 每 turn 从 provider 免费返回，现在只喂成本统计
  （pricing.rs）+ goal 预算（engine.rs:1922）+ footer，**从不回灌估算器**。
- 做法：① 回灌校准 k（`observed = real / raw_estimate`，EWMA 平滑）；② 让正常压缩
  也用 `estimated_input_tokens()`（全量）判据，与 E 同尺 → T/E 退化成同轴两个百分比，
  规则变一句话「input ≥ 75%W 温和压，≥ E 强制压」，S/M/R/档位全部消失。
- 坑：冷启动首请求无历史 usage（默认系数兜底，呼应 warmup）；prefix cache 命中时
  `prompt_tokens` 含 cached+uncached 要用总数；压缩后消息集突变需变化检测；①②必须
  一起做（回灌校准全量估算器，只回灌不改判据则 T 那半把尺没校准到）。
- 落地后：把一期的 T 从公式常数改成 75%W，删 S/M/R；app 侧只剩「探测填 context_tokens」。

## 5. 不做什么（及重新考虑的信号）

256K 大窗口 + 业务会话不长 → 压缩是**尾部事件**，为它上重机制 ROI 撑不起：

| 方案 | 为什么现在不值 | 重新考虑的信号 |
|---|---|---|
| layered seam（底座已有，默认关） | 复杂 + 每次多请求，底座自己没敢默认开 | 出现超长自主 agent 循环，压缩成热路径 |
| 会话 RAG（pinvou 有 L1 基础） | 对话连续性/指代不能靠检索 | 同上 |
| 结构化状态（todo/notes） | 底座已部分覆盖，业务非长循环 | 同上 |
| vLLM `/tokenize` 精确计数 | 每 turn 多一次往返 + pinvou 专用 | 仅作回灌异常时的对账探针 |

## 6. 实证结果（2026-07-02，已完成）

harness：一次性对拍程序（已于 2026-08 随测试清理删除；结论已定稿进
`CompactionConfig`，本节即为存档）。
程序化对拍两把尺，不依赖真机/vLLM——一期正确性只取决于两条**估算尺的相对关系**，
与估算准不准无关（真实 token 校准是上游线的事）。

**结论**：
1. **k=1.5 精确**：三档（262K/131K/65K）k_eff 全 =1.500。pinvou 关 thinking，两把尺
   差异就是纯 ×1.5，无 thinking-aware 偏移 → 公式 `T=(E−S)/k−R` 结构成立。
2. **S/R 远小于原假设**：dump 实测 base system ~1,405 conservative token（S 取 4K）；
   R（pinned=近 4 条+query）实测 4,358 且**与会话长度无关**（原假设 15K，高估 3 倍）。
3. **190K 在健康 256K 机也倒置**（§2 已回填）：emergency N=198 先于 should_compact(190K)
   N=255。当前生产一直走 emergency 路径，只因 emergency 也先摘要而未暴露。
4. **T 必须按窗口缩放**：262K→~133K、131K→~46K，写死单值对任一窗口都错（非倒置即过保守）。

触发顺序实证原始数据（真实 `should_compact`，含 MIN_SUMMARIZE=6 等全部细节）：

| W | E | T=190K | T=130K | T=45K |
|---|---|---|---|---|
| 262,144 | 236,544 | ❌ 倒置(sc@255 > em@198) | ✅ sc@174 < em@198 | ✅ sc@60 |
| 131,072 | 105,472 | — | ❌ 倒置(sc@174 > em@84) | ✅ sc@60 < em@84 |

**待真机精修（不阻塞编码）**：S 用实际会话 dump（含 workspace/memory/更多 skills 注入）
校准；但 S 是 /1.5 衰减的线性项，1.4K↔12K 只让 T 差 ~7K，取保守 4K 已够安全。

## 7. 开工顺序

实证已定常数 → 现在可进一期编码（§4）：探测透传 → `active_route_limits.context_tokens`
→ T 推导 helper（用 §3 实测公式替换写死 190K）→ 参数化守护测试（同尺不变式）。
