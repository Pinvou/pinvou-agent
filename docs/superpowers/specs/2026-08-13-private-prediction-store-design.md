# Run-scoped Private Prediction Store 设计

## 背景与目标

`NativeAgentRunner` 当前在 backend session 关闭前把最终回答解析成 `SecretOutput`，但
`RunStore` 只把 backend handle 写入公开 `predictions.jsonl`。Session 关闭后 handle 失效，
`read_outcomes` 也不恢复 `private_output`，因此 official adapter 无法在进程重启或 resume 后
执行 score/submission。

本设计增加 run-scoped 私有预测存储，要求 raw answer 永不进入 manifest、公开 JSONL 或
Markdown report；公开 handle 只是关联标识而非 capability；scorer 只能通过 run-scoped
`ScorerView` 读取；Windows 使用当前用户 DPAPI 加密每个 blob；Unix 在创建时强制目录 0700、
文件 0600；Smoke 默认不持久化回答。

## 非目标

- 不持久化 prompt、ground truth、reference、tool input/output 或 provider error。
- 不把 prediction store 做成 credential store、云同步或跨用户/机器共享。
- 不让 adapter 直接打开私有路径。
- 不存 WorkBuddy 大 artifact/trajectory；它们需要独立 artifact store。
- Submission 是用户显式请求的 answer 导出，不属于公开 report，也不由本存储自动发布。

## 现有能力复用

`CodeWhale/crates/secrets` 的 `FileKeyringStore` 已有 Unix 0700/0600、临时文件、`sync_all`
和原子发布模式，可复用其测试与权限思路。但其 Windows fallback 明确不强制 ACL，且为 JSON
明文，不能直接存 prediction。`codewhale-secrets` 的 OS keyring 适合少量长期 credential，
不适合每个 run 的大量 blob，也未暴露通用 DPAPI blob API。

因此不新建第二套 credential store，也不把每条 answer 塞进 keyring。Windows 在 core 的窄
平台模块中复用仓库已有 `windows-sys` 依赖族，直接调用 `CryptProtectData` /
`CryptUnprotectData`；Unix 复用既有私有文件模式。不得调用 PowerShell 或外部加密命令。

## 威胁模型

防护误写公开报告、其他本机用户读取、路径穿越、符号链接替换、crash 半写、handle 重放、
跨 run 替换、并发 writer、磁盘耗尽与错误回显。不防护已控制当前用户进程或管理员/root。
Unix 0600 明文是 MVP 的明确边界；Windows DPAPI 也不防同一用户上下文中的恶意进程。

公开 handle 不授予读取权。读取还必须持有 core 构造的 `ScorerView`，并匹配 run ID、task
ID、prediction type 与 handle。任何失败只返回固定 code，不回显路径、handle、answer、
DPAPI error 或解密内容。

## 公共契约

`BenchmarkAdapter` 增加安全默认方法：

```rust
fn private_output_retention(&self) -> PrivateOutputRetention {
    PrivateOutputRetention::Ephemeral
}
```

`PrivateOutputRetention` 只有：

- `Ephemeral`：只在本次执行内存中使用，公开 outcome 不生成可恢复 prediction；
- `DurableUntilPurge`：先写 private store，再写公开 outcome，支持恢复后 score/submission。

Smoke 使用 Ephemeral。GAIA/BFCL official adapter 显式使用 DurableUntilPurge。Core 不按
benchmark 名称硬编码策略。

私有 payload 是 bounded bytes，不实现 serde/Clone，Debug 固定脱敏：

```rust
pub struct PrivatePredictionPayload {
    content_type: PrivatePredictionContentType,
    bytes: Zeroizing<Vec<u8>>,
}

pub enum PrivatePredictionContentType { Utf8Text, CanonicalJson }
```

单条明文上限 1 MiB。GAIA 使用 UTF-8，BFCL 使用 adapter 已验证的 canonical JSON。内容允许
credential-like 字符串；不得用 marker 扫描拒绝合法答案，隔离而非文本过滤负责安全。

最小 store 接口：

```rust
pub trait PrivatePredictionStore: Send + Sync {
    fn put(&self, run_id: &str, task_id: &str, prediction_type: &str,
           payload: PrivatePredictionPayload) -> Result<PredictionHandle>;
    fn scorer_view(&self, run_id: &str) -> Result<ScorerView>;
    fn garbage_collect(&self, run_id: &str, live: &LivePredictionSet) -> Result<GcSummary>;
    fn purge_run(&self, run_id: &str) -> Result<()>;
}
```

`ScorerView::resolve(&TaskOutcome)` 从 outcome 取得 task/type/handle 并校验三者绑定，不公开
路径或 handle-only API。只有 core 能构造它。`CompletedRun` 持有可选 scorer view；普通
`CompletedRun::new` 无 capability，解析固定返回 `private_prediction_unavailable`。

## 文件布局与保护格式

```text
<run>/
├── manifest.json
├── events.jsonl
├── predictions.jsonl
├── .run.lock
└── private/predictions/
    ├── <256-bit-lower-hex>.blob
    └── .<256-bit-lower-hex>.<pid>.tmp
```

Handle 由 OS CSPRNG 生成 256 bit，不包含 run/task/session/provider 信息。公开 JSONL 仅保存
core handle 与安全 type tag；backend 原始 handle 永不落盘。

Blob 是版本化二进制 envelope，含 magic、schema、content type、绑定摘要、明文长度和
payload。Windows payload 为 DPAPI ciphertext；optional entropy 由固定 domain separator 与
run/task/type/handle 的长度前缀编码做 SHA-256 得到，阻止跨 run/task 替换。Unix payload 为
0600 明文，并保存 SHA-256 只用于偶发损坏检测，不宣称防恶意篡改。未知 schema、截断、超限、
绑定不符和解密失败统一返回 `private_prediction_corrupt`。

## 平台策略

Windows：使用当前用户 `CryptProtectData`/`CryptUnprotectData`，设置
`CRYPTPROTECT_UI_FORBIDDEN`，禁止 machine scope 与 description；`DATA_BLOB` 使用
`LocalFree`，明文 scratch 尽快 zeroize。DPAPI 不可用时 durable run fail-closed 为
`private_protection_unavailable`，绝不降级明文。MVP 选择每 blob DPAPI，避免新增 AEAD、nonce
与 per-run DEK 生命周期；若性能成为实测瓶颈，再以 schema v2 引入 DPAPI-wrapped DEK。

Unix：私有目录创建并验证 0700；temp/final 以 `OpenOptionsExt::mode(0o600)` 在创建时设权，
读取前再次拒绝 group/world bit、symlink 与非普通文件。不得依赖 umask 或 best-effort chmod；
无法证明权限语义的文件系统返回 `private_permissions_unsupported`。

## 原子性与锁

每个 run 的 event/outcome/private mutation 共用 `.run.lock` 的 OS advisory exclusive lock；
进程内 mutex 不能替代 `fs2` 进程间锁。Durable 提交顺序固定为：

1. 获取 run lock；
2. 生成 core handle并保护 blob；
3. `create_new` temp、私权限写入、flush、`sync_all`；
4. no-clobber 原子发布 final，Unix 同步父目录；
5. append + `sync_data` 公开 outcome；
6. append + `sync_data` terminal event；
7. 释放 lock。

失败不得写 Completed terminal。Private durable/public outcome 前 crash 只产生 orphan；outcome
durable/terminal 前 crash 由 reducer 恢复。公开 Completed 指向缺失或损坏 blob 时 score /
submission fail-closed，不自动重跑，以免产生不可复现新答案。

## GC、配额与清理

恢复时在 exclusive lock 下从公开 outcomes 构建 live handle 集合。GC 只处理
`private/predictions` 直属目录中满足内部精确命名规则的 blob/temp；删除 orphan 与遗留 temp。
遇到未知文件、子目录、symlink/reparse point 返回 `unsafe_private_store_layout`，不得递归删。

固定配额：单条明文 1 MiB、单 run 10,000 条、单 run envelope/ciphertext 总量 100 MiB；temp
和 final 均纳入写前检查。Durable 默认保留到用户显式 purge。`purge_run` 验证精确 run ID 与
canonical containment，逐文件删除已识别 blob，再删除空私有目录；绝不递归删除 run、base、
home 或 repository root。TTL 清理由后续独立 proposal 决定。

## Adapter 行为

- GAIA validation 从固定 dataset revision 的 scorer-only reference 读 ground truth，从
  ScorerView 读 UTF-8 candidate；report 只写 aggregate score。
- GAIA private test 无本地 ground truth 时返回 `local_scoring_unavailable`，显式 submission
  仍可读取 candidate。
- BFCL 从 ScorerView 读 canonical JSON；解析错误不回显 JSON。
- Smoke 保持 Ephemeral；确定性 Product Health 只依赖 status/tool/usage/latency。当前 Judge
  NotConfigured 不改变策略。

## 验收门禁

- 公开目录扫描不含 raw answer、backend handle 或 DPAPI error 文本。
- 单独 public handle 不能 resolve，Debug/serde/error 不泄露 secret/path/handle。
- Windows 覆盖 DPAPI round-trip、错误 entropy、跨 run swap、损坏 ciphertext、UI forbidden。
- Unix 覆盖 0700/0600、过宽权限、symlink 和非普通文件拒绝。
- 覆盖 durable→outcome→terminal 顺序的 crash fixtures、进程间锁和恢复评分。
- 覆盖 Ephemeral Smoke 不创建 blob、orphan/temp GC、未知布局、配额和精确 purge。
- Submission 是唯一显式 answer 导出；manifest、JSONL、report 与 score envelope 只含安全汇总。

只有这些门禁通过，official adapter 的 resume/score/submission 才能标记可用。
