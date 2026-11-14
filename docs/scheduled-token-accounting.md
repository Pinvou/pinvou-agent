# 定时会话令牌核算（EngineCumulative）

> 英文镜像：[`scheduled-token-accounting.en.md`](./scheduled-token-accounting.en.md)（按 CONTRIBUTING 对新开发者文档的英文要求补充；两文内容一一对应，分歧以中文原文为准）。

2026-11-12 与官方构建对齐（上游提交 `624c384941`）时，会话侧只取核算内核：`ScheduledTokenAccounting::EngineCumulative { base_total_tokens, engine_total_tokens }` 变体，以及 `persist_scheduled_engine_state` 的配对匹配。生存期总额统一经 `scheduled_lifetime_total` 计算（base + engine 饱和相加），`persist_scheduled_engine_state` 与 `persist_scheduled_token_total` 共享同一编码，合并结果替换持久化总额而非累加。

生产者两侧均刻意未接线：本树与官方构建都没有在测试之外构造该变体的代码，分裂系列（#856–#868）也没有任何一个 PR 接线。接线只需要应用侧的一个后续 PR——spawn 基线已在 `assistant/engine.rs` 于每次 spawn 时从持久化总额读取，跨回合累计已在转发器 `TurnComplete` 分支累加，生产者只需在 `SessionUpdated` 分支改构造 `EngineCumulative`。

官方构建同批对齐改动中涉及 workspace 字段的半块改动（未进入公共对齐提交 `624c384941`）被判定为破坏性，刻意不取：`session.metadata.workspace` 保持由 profile 派生的既有语义。

核算合同：`engine_total_tokens` 是该引擎自 spawn 起的累计用量，不是逐次增量；合并结果替换持久化总额，过期的 base 允许把总额调低，因此每次重 spawn 都必须从持久化总额重读基线。饱和钳制与"替换而非 max()"的语义均由回归测试钉住（base + engine 溢出饱和于 `u64::MAX`；过期基线 `{100, 5}` → 105）：

```sh
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib scheduled_engine
```
