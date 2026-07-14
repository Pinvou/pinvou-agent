# Qwen3.6 + vLLM prefix caching / MTP 工具调用漂移复盘

> 复盘日期：2026-07-14
> 环境：GB10 / 113，Qwen3.6-35B-A3B-FP8，vLLM OpenAI-compatible server
> 状态：生产配置已保留 prefix caching、关闭 MTP；针对性回归通过，全量 L1 因 113
> 整机失联只完成 21/27，待设备恢复后补齐

## 1. 结论先行

本次问题不是 skill、单个工具实现、QCC 或天气 Schema 的确定性 bug，也不是客户端把
正确的 tool call 解析丢了。

当前证据支持的高置信结论是：

> 113 当前 Qwen3.6 / vLLM 构建中，MTP-2 与实验性的 Mamba/GDN prefix cache `align`
> 路径发生交互，产生了与 cache namespace 相关的异常生成状态；在
> `tool_choice=auto` 没有 JSON grammar 约束时，异常最终表现为工具调用格式漂移。

典型表现是模型明明应该调用 `write_file`，却输出普通文本、裸 JSON 或伪
`write_file(...)` 文本。parser 收到的本来就是非协议内容，无法还原成标准
`tool_calls`。

最终采取的最小影响方案：

- 保留 `--enable-prefix-caching`；
- 删除 `--speculative-config`，即关闭 MTP；
- 不修改 skill、工具 Schema 或客户端 parser 作为兜底；
- 不恢复 always-on warmup。

关闭 MTP 后，同一份此前稳定失败的 41 工具请求从 **10/10 失败**变成
**10/10 标准 `write_file` 调用**。恢复 QCC 后，真实 Engine/L1 落盘场景也通过。

## 2. 故障现象

真实 L1 自动化中，要求模型使用 `write_file` 创建文件。应用发送的是标准 OpenAI
Chat Completions 请求，`tool_choice=auto`，请求中包含当前对模型可见的工具 Schema。

失败时出现过三类输出：

1. 普通文本声称“文件已创建”，实际没有工具调用；
2. 普通 content 中输出 `{ "name": "write_file", "arguments": ... }`；
3. 输出类似 `write_file(path=...)` 的伪调用文本。

共同点：服务端 SSE 的 `delta.tool_calls` 为空，最终 `finish_reason=stop`。因此问题发生在
应用解析之前，不能靠修改 DeepSeek-TUI parser 根治。

## 3. 运行环境与原始配置

113 当时同时启用：

- Qwen3.6-35B-A3B-FP8；
- 256K context；
- prefix caching；
- MTP-2：`num_speculative_tokens=2`；
- FP8 KV cache；
- chunked prefill；
- async scheduling；
- `tool_choice=auto` + Qwen tool-call parser。

vLLM 启动日志把模型架构解析为 `Qwen3_5MoeForConditionalGeneration`，并提示 Mamba
cache `align` 模式下的 prefix caching 仍属实验支持。

## 4. 关键实验矩阵

### 4.1 应用之外重放原始请求

我们先捕获 pinvou3 实际发送的完整请求，再绕过应用直接请求 vLLM，排除了 bridge、
Engine 事件处理和客户端 SSE parser。

| 对照 | 结果 | 说明 |
|---|---:|---|
| 默认 cache namespace，原始 41 工具请求 | 10/10 错误格式 | 可稳定复现 |
| 每次使用新的 `cache_salt` | 10/10 标准 tool call | prompt 内容不变，只隔离缓存空间 |
| 固定一个新的 `cache_salt` 重复 | 5/5 标准 tool call | 后续已命中 8,512 cached tokens，说明“缓存命中本身”不必然失败 |
| 默认 namespace + 固定 seed | 5/5 错误格式 | 排除普通采样噪声 |

`/render` 结果证明有无 `cache_salt` 时：

- 输入均为 11,928 tokens；
- token hash 一致；
- `temperature=1.0`、`top_p=0.95`、`top_k=20` 一致；
- `cache_salt` 不进入模型提示词，只改变缓存块命名空间。

这说明失败与缓存空间中的状态有关，不是用户 prompt 或工具 Schema 被 `cache_salt`
改写。

### 4.2 `tool_choice` 约束强度

在同一个异常 cache namespace 下：

| `tool_choice` | 正确结构化调用 |
|---|---:|
| `auto` | 0/5 |
| `required` | 4/5 |
| named `write_file` | 5/5 |

`/render` 同时显示：

- `auto` 的 `structured_outputs=null`，仍是自由采样；
- `required` 才启用全工具 JSON grammar；
- named tool 才启用单工具 grammar。

因此 `tool_choice=auto` 不是底层异常的来源，但它没有约束解码，是异常状态最终逃逸成
裸文本/裸 JSON 的关键放大器。

### 4.3 关闭 MTP、保留 prefix caching

只改 vLLM 启动配置：删除 MTP-2，其他关键参数保持不变。

| 验证 | 结果 |
|---|---:|
| 启动日志 | `speculative_config=None` |
| prefix caching | `enable_prefix_caching=True` |
| `/v1/models` | HTTP 200 |
| 此前 10/10 失败的 41 工具请求 | 10/10 标准 `write_file` |
| 真实 L1，QCC 临时禁用 | `write_file` 1 次，文件落盘，约 5.7s |
| 真实 L1，QCC 恢复启用 | `write_file` 1 次，文件落盘，13.6s，1 passed |
| 全量 L1，QCC 启用 | 前 21/27 场景通过；RAG 子代理场景末尾流读取断开，随后 113 的 SSH/HTTP 同时超时，后 6 条被 harness 跳过，不能记为全量通过 |

最后一条回归 transcript：

- `pinvou3-app/src-tauri/target/l1-runs/1784003373/save_to_tmp_no_validate_fail.md`

该文件位于 ignored build artifact 中，不作为长期证据提交；长期证据以本文实验数据和可重复
运行的 L1 scenario 为准。

2026-07-14 的全量运行 transcript 位于
`pinvou3-app/src-tauri/target/l1-runs/1784004245/`。其中前 21 个场景没有断言失败；
`subagent_research_topic` 在 99.9 秒处收到已产生部分输出后的 stream decode error，随后
113 的 22/8000 端口和 ICMP 均不可达。`require_vllm()` 会把端点不可达后的 scenario
作为 `SKIP ... ok` 返回，所以最终 `test result: 27 passed` **不等于 27 个场景都实际跑过**。
本次只能记录为 21/27，设备恢复后需补跑剩余 6 条并检查重启/掉线原因。

该次运行后已加固 harness：`PINVOU3_L1_REQUIRE_VLLM=1` 不仅让 pre-flight 不可达
直接失败，还会把 turn 内 `Event::Error` 收集进 `TurnSummary` 并断言为空；对应纯函数回归
进入 Rust CI。以后同类 stream decode error 不会再被 Cargo 计成通过。

## 5. 因果链与置信边界

### 已实证

1. 错误输出来自服务端生成，不是客户端 parser 丢失 tool call。
2. 完全相同 token 序列在不同 cache namespace 中出现稳定的成功/失败分化。
3. 新 namespace 后续命中缓存仍正确，所以不能简单归纳为“prefix cache 一命中就坏”。
4. 关闭 MTP、保留 prefix caching 后，原失败请求和真实落盘场景均恢复。
5. QCC、工具数量、天气 Schema、温度会改变触发概率，但都不能独立解释全部现象。

### 高概率机制

结合 113 日志和 A/B，最可能的链路是：

```text
实验性 Mamba/GDN prefix cache align 路径 + MTP-2
  → 某个 cache namespace 中形成与非 MTP 路径不一致的可复用生成状态
  → 相同长前缀持续复用该状态
  → tool_choice=auto 自由采样脱离工具协议
  → vLLM parser 只能返回普通 content，无法生成 delta.tool_calls
```

### 尚未严格证明

- 尚未定位到 vLLM 内部具体是哪一个 Mamba state、block matching 或 speculative decode
  状态发生不一致；
- 113 当前 vLLM build 没有暴露 cache reset API，因此没有完成“只清默认 namespace、
  不重启模型”的直接验证；
- 没有执行“恢复 MTP、只关闭 prefix caching”的完整自动化 A/B，因为 pinvou3 是长输入、
  短输出负载，关闭 prefix 的性能损失明显大于先关闭 MTP。

所以本文把“具体底层缓存对象如何损坏”标为机制推断，但把“当前环境中关闭 MTP 能消除
该问题”视为已验证的工程结论。

## 6. 被排除或降级的假设

### QCC / 工具过多

禁用 QCC 后，对模型可见的工具从约 148 降到 41，失败概率发生变化，但同类问题仍能
复现。QCC 会显著放大 prompt 和 logits 扰动，不是底层根因。验证完成后 QCC 已恢复启用。

### 天气 Schema

天气工具描述中嵌套 JSON 字符串和数组示例，确实会扰动弱模型格式；短样本中修改它曾让
场景恢复。但随后同一原始 Schema 配合新 `cache_salt` 10/10 成功，只保留
`write_file` 也曾失败，因此它只是概率扰动项。

### temperature

把 temperature 改为 0 没有稳定消除问题，部分场景反而稳定进入相同错误循环。

### warmup

历史 warmup 用“同前缀 + `max_tokens=1` + `tool_choice=none`”抢先填 prefix cache，曾在
一段时间内缓解首轮漂移。但它只能提高“先写入一份好状态”的概率，不能保证缓存状态正确；
默认 namespace 连续失败也证明“缓存热了”不等于正确。

因此 warmup 不是根治，不应恢复为 always-on 机制。它还会增加首轮请求、复杂化 cancel/
错误处理，并掩盖服务端真实问题。

## 7. 为什么选择关闭 MTP，而不是关闭 prefix caching

诊断时的负载快照：

- prefix cache prompt token 命中率约 79%；
- MTP-2 draft token 接受率约 88.4%；
- 平均输入约 21,900 tokens；
- 平均输出约 323 tokens。

pinvou3 是明显的“长输入、短输出”负载：系统提示、工具表和历史上下文远大于单次输出。

真实 11,928-token 短工具请求中：

- 缓存命中首包平均约 1.28s；
- 冷 prefill 平均约 2.61s。

关闭 prefix caching 会让每轮重新计算长前缀，短工具调用约多 1.33s，并降低吞吐；关闭
MTP 主要影响输出阶段，对短工具调用影响更小。因此优先保留 prefix caching、关闭 MTP。

额外观察：关闭 MTP 后模型占用由约 21.94 GiB 降到约 20.37 GiB，释放约 1.6 GiB。
长报告和长代码的输出速度损失尚未做正式 benchmark，不应从短工具场景外推具体比例。

## 8. 当前生产配置

113 持久启动脚本：

```text
/opt/.h3c/packages/vllm/start_nvfp4.sh
```

修改前备份：

```text
/home/whc0005/start_nvfp4.sh.bak-20260714-121052
```

当前要求：

- 脚本中保留 `--enable-prefix-caching`；
- 脚本中不应出现 `--speculative-config`；
- 启动日志应显示 `speculative_config=None`；
- 未来重装/升级 vLLM 包时必须复核持久脚本没有恢复 MTP。

只读核对：

```bash
ssh whc0005@10.214.74.113 \
  "grep -n -E -- '--enable-prefix-caching|--speculative-config' \
  /opt/.h3c/packages/vllm/start_nvfp4.sh"

curl -fsS http://10.214.74.113:8000/v1/models | jq .
```

## 9. 回归方法

最小真实落盘回归：

```bash
DEEPSEEK_BASE_URL=http://10.214.74.113:8000/v1 \
DEEPSEEK_MODEL=qwen36_35b_256k \
PINVOU3_L1_REQUIRE_VLLM=1 \
cargo test \
  --manifest-path pinvou3-app/src-tauri/Cargo.toml \
  --test l1_dialog_harness \
  save_to_tmp_no_validate_fail \
  -- --ignored --exact --nocapture --test-threads=1
```

通过条件：

- `tool_call_histogram` 为 `{"write_file": 1}`；
- `timed_out=false`；
- test result 为 `1 passed; 0 failed`；
- transcript 中同时存在 `tool_start write_file` 和 `tool_end ... ok`。

发版或 vLLM 升级后的建议验证：

1. 连续运行该场景 10 次，检查是否有普通文本假成功；
2. 跑完整 ignored L1 harness；
3. QCC 保持启用，避免只验证缩小后的工具面；
4. 检查 vLLM 日志和 metrics 中没有 `spec_decode` 指标；
5. 对比 prefix cache hit rate、TTFT、长输出 tokens/s 和显存占用。

## 10. 未来重新启用 MTP 的门槛

只有满足以下条件才重新评估 MTP：

1. vLLM release note 明确修复 Qwen3.5/3.6 Mamba prefix cache 与 MTP 交互；
2. 先在非生产实例恢复 MTP，不能直接改 113 生产配置；
3. 原始失败请求至少 100/100 结构化调用；
4. 完整 L1 harness 全过，并包含 QCC 启用的完整工具面；
5. 相同 seed、固定/不同 cache namespace、冷/热 prefix 都通过；
6. 输出速度收益足以覆盖新增稳定性风险。

如需低延迟试验，优先测试官方建议的 MTP-1，而不是直接恢复 MTP-2。

## 11. 外部佐证

- [vLLM 官方 Qwen3.5 & Qwen3.6 Usage Guide](https://github.com/vllm-project/recipes/blob/main/Qwen/Qwen3.5.md)：明确说明 Mamba cache `align` 模式的 prefix caching 仍是 experimental；低并发低延迟配置建议 MTP-1 且不启用 prefix caching。
- [vLLM Issue #38182](https://github.com/vllm-project/vllm/issues/38182)：公开记录 Qwen3.5-35B-A3B 启用 MTP 后 prefix cache hit rate 从约 92% 降到约 71%。该 issue 只能作为交互异常的旁证，不直接证明本文的工具调用漂移机制。
