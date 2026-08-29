# GAIA 本地评测

GAIA 本地评测位于独立的 `pinvou-cli` workspace，只在显式构建 CLI 时启用评测后端；桌面应用的默认 feature、启动路径和发行安装包均不包含该入口。未运行 `pinvou benchmark ...` 时，评测不会创建会话、加载数据集、注册工具策略或改变普通聊天的运行时与持久化语义。

该 CLI 用于在受支持的平台运行 GAIA validation Level 1 任务集并生成官方格式的提交文件。评分器是固定版本的 Rust 移植，尚未完成与官方 Python scorer 的逐题 parity 验证，因此本文不宣称评分结果与官方实现等价。Windows 当前受附件执行安全门禁限制，不能视为可完成 GAIA 端到端实跑的平台。

从源码构建入口：

```bash
cargo build --manifest-path pinvou-cli/Cargo.toml --bin pinvou
```

## Access and gating

GAIA 数据集托管在 Hugging Face 仓库 `gaia-benchmark/GAIA`。该仓库为 **gated**：必须先在 Hugging Face 网站申请访问权限并生成 read access token。

- Token 通过环境变量传入，**绝不会**写入命令行参数、配置文件或日志。
- 使用 `--token-env HF_TOKEN` 指定存放 token 的环境变量名；适配器在读取后立即用于 HTTPS 下载，不持久化、不回显。
- 适配器**不读取浏览器登录态**，不依赖任何本地 cookie 或凭据缓存。
- token 环境变量名非法、缺失或值为空时返回 `gaia_access_denied`；Hugging Face 拒绝访问或下载失败统一返回 `gaia_download_failed`，两者都不泄露 token、路径或底层响应细节。

远程元数据或内容下载失败返回 `gaia_download_failed`；本地导入源的路径或附件安全约束不合规返回 `gaia_import_failed`；数据集固定内容、已发布 ready marker、完整性清单或文件内容校验失败返回 `gaia_verify_failed`。

同一数据目录已有 fetch/import 正在运行时返回 `gaia_fetch_in_progress`；锁由操作系统持有，进程崩溃后自动释放，遗留锁文件可安全复用。

访问权限审批由 Hugging Face 平台管理，Pinvou 无法代为申请。

## Pinned revisions

为保证评测可复现性，所有关键资源均固定到精确版本：

| 资源 | 固定值 |
|------|--------|
| 数据集 Git revision | `682dd723ee1e1697e00360edccf2366dc8418dd9` |
| 评分器 Git revision | `1349a17979f0aca0ee9c46cd7ec26eb2fb41102e` |
| 适配器版本 | `pinvou-gaia-adapter/v1` |
| 评分运行时 profile | `hf-spaces-python-3.10-unicode-13.0` |
| Parquet 文件 | `2023/validation/metadata.level1.parquet` |
| Parquet 大小 | 39,524 字节 |
| Parquet SHA-256 | `5e574b0faeb4603b816e426cf7c7aefb1fe398d32f9c4861e1a4e3304f2b1281` |
| Ready schema | `pinvou-gaia-ready/v1` |
| 附件完整性清单 | `.pinvou-gaia-integrity-v1`（JSONL，SHA-256） |
| 数据集 split | `validation` |
| 数据集 level | `1` |

下载阶段校验 LFS 对象的 SHA-256、非 LFS 对象的 Git blob SHA-1，以及每个文件和累计传输预算；导入阶段校验 Parquet 文件的 SHA-256、尺寸和 revision marker。发布时为数据集引用的每个附件写入 `.pinvou-gaia-integrity-v1`，ready marker 最后写入并绑定该清单的 SHA-256。以后每次打开 ready 快照都会重新校验 marker、清单、附件集合、大小和内容摘要，任何不匹配都 fail closed。

## Fetch or import

提供两种获取数据集快照的方式：

### 通过 Hugging Face 远程拉取

```bash
export HF_TOKEN="hf_xxx"
pinvou benchmark fetch gaia --token-env HF_TOKEN
```

- 仅接受 `gaia-benchmark/GAIA` 仓库、固定 revision `682dd723...`，其他仓库或 revision 一律拒绝。
- 流式下载，累积大小超限或文件数超限时在写入前中止。
- 下载完成后先写入附件完整性清单，最后写入绑定该清单摘要的 ready marker（`pinvou-gaia-ready/v1` schema），并设置私有文件权限（Unix 0600；Windows 使用受保护 DACL，仅为当前用户、SYSTEM 和 Administrators 配置完全控制）。
- 已存在 ready 目录不会被覆盖（no-clobber）。

### 从已有快照导入

```bash
pinvou benchmark fetch gaia --source /path/to/private-snapshot
pinvou benchmark verify gaia --source /path/to/private-snapshot
```

- `fetch --source` 直接从本地目录导入并发布 ready marker。
- `verify --source` 校验快照的 SHA-256、尺寸和 revision marker，但不发布 ready marker（只读校验）。
- 快照目录中的符号链接和 reparse point 一律拒绝。
- `97008045` 起，旧版 ready marker 因未绑定 `.pinvou-gaia-integrity-v1` 会被严格拒绝；已有快照必须重新执行 `fetch` 或从可信源重新 `import`，不能手工补写 marker。

## Validation Level 1

GAIA validation Level 1 是当前唯一支持的 split/level 组合。每道题包含一个问题文本和零或多个附件引用。

运行评测需要 product-backend 能力：

```bash
pinvou benchmark run gaia --split validation --level 1
```

- 每道题有 600 秒超时限制。
- 代理使用 `pinvou-gaia-public-web/v1` 工具策略，可访问公开 web 资源。
- Office/PDF 附件在 host 侧预解析；XLSX 会同时提供工作表值以及有界的填充色、公式和合并
  区域注释。评测附件提示只声明 profile 实际允许的只读能力。
- 输出契约为 `gaia-final/v1`：题目 prompt 会注入最终答案格式指令
  （`FINAL ANSWER: <answer>`）。解析按行、大小写不敏感，并容忍常见 Markdown 强调；只提取最后一个已识别标记所在行的内容。最后一个标记为空时任务以 `missing_final_answer` 终态计为失败，不回退到更早标记或正文。
  如果首轮已完成但缺少该标记，或首轮以 Agent 工具失败结束，评测后端会追加一次禁用全部工具的最终答案恢复轮；该轮只允许模型按同一格式给出答案，不会再次执行工具。两轮的模型用量、请求性能和工具观测会合并进入同一道题的诊断。恢复轮仍失败时保留两轮合并诊断，并将该题按失败原因记为 `Failed` 或 `Timeout` 终态。
  预测以 `utf8-text/v1` 持久化在该 run 的私有目录中。当前 CLI 尚未提供 purge 子命令；需要清理时必须在确认 run 已停止后删除对应的 `~/.pinvou3/eval/runs/<run-id>/`，不要删除整个运行时根目录。
- **验证集污染警告**：validation split 的参考答案用于评分。在运行期间不要向代理泄露参考答案、不要用 validation 题目做 prompt 调试，否则评分无效且不可复现。
- 未启用 product-backend 时返回 `product_backend_not_enabled`。
- Windows 当前无法安全挂载 headless 附件：含附件任务在执行前返回 `attachments_platform_security_unsupported`。在该门禁解除并完成真机验证前，不应把 Windows 描述为支持 GAIA 端到端实跑。

### 查看运行状态和报告

```bash
pinvou benchmark list                          # 列出注册的 benchmark
pinvou benchmark status <run-id>               # 查看运行状态
pinvou benchmark score gaia --run-id <run-id>  # 评分；完整 run 才落 report.md / score.json
pinvou benchmark report <run-id>               # 查看完整 run 的 report.md（需先完成 score）
pinvou benchmark resume <run-id>               # 恢复未完成的运行（product-backend）
```

- 同一 `run-id` 只允许一个执行者持有运行锁；并发 `run` / `resume` 返回 `run_in_progress`，避免同一 task 被重复执行和落盘。
- 进程在追加 JSONL 时崩溃留下的末行残片会在恢复前截断；完整但缺换行的末行会补齐换行。已换行的损坏记录、未知 schema 或重复 task outcome 都会 fail closed，不会被静默当成可恢复状态。

### 评分

```bash
pinvou benchmark score gaia --run-id <run-id>
```

- 评分器从持久化的私有预测中解析候选答案，与参考答案比对。
- 未完成 run 的 score 只返回 `unofficial_partial` 诊断，不发布固定的
  `score.json` / `report.md`；resume 完成后再次 score 才发布最终产物，避免 partial
  结果占用固定文件并阻塞最终评分。
- Rust 评分器是基于 revision `1349a179...` 的固定移植，包括数字归一化、字符串/列表归一化、Unicode 十进制数字、Python 全串小写和控制空白符处理；golden contract 只覆盖这些已知规则。
- 评分运行时 profile 为 `hf-spaces-python-3.10-unicode-13.0`。
- 真实 Python scorer 逐题交叉验证尚未完成；在完成该验证前，golden contract 通过不等同于已经证明整套 validation Level 1 的逐题结果等价。

## Official scorer compatibility

评分报告使用 `OfficialScoreReport`，区分两类：

| 标签 | 含义 |
|------|------|
| **complete / pinned Rust port** | 每个题目都有且仅有一个持久化终态；失败、超时、取消和缺失答案均计为错误 |
| **partial / structurally incomplete** | 存在缺题、重复、未知题目、损坏预测或仍在运行等结构问题 |

报告字段包括 `evaluated`、`correct`、`complete`、`official_dataset_compatible`、`split`、`level`。

- complete 报告的 `split` 为 `validation`、`level` 为 `1`。
- CLI 的完整状态为 `complete_pinned_rust_port`；它表示结构完整，不表示已经证明与官方 Python scorer 等价。
- partial 报告明确标注为结构不完整、不可比。
- 评分器只读取持久化的私有预测；公开预测句柄无法解码候选答案。

## Submission

```bash
pinvou benchmark submission gaia --run-id <run-id> --destination ./output.jsonl
```

- 生成官方提交 JSONL 文件：每行一个 compact JSON 对象，恰好包含 `task_id` 和 `model_answer` 两个键。
- 行序与数据集 Parquet 行序一致（确定性）。
- 接受结构完整的运行：每道题必须有且仅有一个终态结果。成功任务写入持久化答案；失败、超时、取消或 `missing_final_answer` 写入安全的空字符串 `"model_answer":""`，由评分器计为错误。缺题、重复、未知题目、损坏预测或 running/planned 状态仍拒绝导出。
- 原子发布：先写同名临时文件（私有权限），sync 后 `hard_link` 到目标路径。目标已存在时返回 `gaia_submission_target_exists`，绝不覆盖。
- 目标文件或叶子父目录是符号链接/reparse point 时返回安全错误；系统路径中的 symlink 祖先（例如 macOS `/var`）允许通过。防覆写由私有临时文件、`create_new` 和最终 `hard_link` 保证。
- 文件中**绝不包含**问题文本、参考答案、附件路径、token、会话 ID 或内部句柄。

## Privacy

- **参考答案隔离**：参考答案仅存在于适配器内部，评分时通过 run-bound scorer view 解析私有预测，不暴露给代理、不写入日志、不出现在提交文件中。
- **附件隔离**：附件通过 `AttachmentHandle` 挂载到沙盒，代理只能访问附件内容，不能访问参考答案或元数据中的私有字段。
- **Token 隔离**：HF token 仅在 fetch 阶段用于 HTTPS 认证，不持久化、不回显、不出现在任何输出中。
- **预测句柄隔离**：公开预测句柄（`PrivateInputHandle`）无法解码候选答案；只有 run-bound scorer view 能解析。新写入的私有预测使用 `PVP3` envelope；reader 继续兼容已发布的 `PVP2` envelope，但两种格式都必须先用绑定运行元数据和受保护正文的摘要完成完整性校验，再解密正文。Windows 预测正文使用 DPAPI 保护；预测目录和 blob 使用与数据集相同的受保护私有 DACL。
- **测试边界**：测试代码不硬编码真实参考答案或真实 token；测试用的私有预测通过 `test-support` feature 下的辅助方法注入，不泄露到默认构建。

## Not a leaderboard score

本地 GAIA 评测结果**不是**官方 leaderboard 分数：

- 本地评分使用固定到官方 scorer revision 的 Rust 实现，并由 golden contract 锁定已知归一化规则；真实 Python scorer 的 validation Level 1 逐题交叉验证仍是待办，运行环境（OS、网络、工具版本）也可能与官方评测平台不同。
- 报告标注 `validation` split，明确这是验证集结果，不是 test set 分数。
- complete 结果只能标注为 "validation Level 1, pinned Rust port local accuracy"，并注明 parity 未验证。
- **禁止**将本地结果描述为 "leaderboard score"、"test score" 或省略 "validation"/"local" 限定词。
- 适配器**不会自动上传**提交文件或分数到任何平台；提交文件生成在本地，由用户自行决定如何使用。

## Known platform limits

- **product-backend 依赖**：运行评测需要 product-backend 能力（真实代理执行）。社区版或未配置 product-backend 的构建中，`run gaia` 返回 `product_backend_not_enabled`。
- **附件平台支持**：Windows headless 附件执行当前固定返回 `attachments_platform_security_unsupported`；因此 Windows 不能宣称支持完整 GAIA 评测。Unix 等平台仍需以实际 product-backend 验证结果为准。
- **网络访问**：运行期间代理可访问公开 web（`pinvou-gaia-public-web/v1` 策略），但不保证特定网站可用性；网络故障导致的超时由 600 秒限制处理。
- **HF gated 访问**：数据集访问权限由 Hugging Face 平台管理，可能因审批延迟或拒绝而无法下载。
- **磁盘占用**：完整数据集快照约 40 KB Parquet + 附件（附件大小取决于题目）；下载阶段有流式大小限制，超限中止。
- **评分等价性边界**：Rust 评分器固定实现和 golden contract 不是逐题等价证明；真实 Python scorer cross-check 未完成，Python/Unicode 运行时差异也可能导致结果不同。以固定 revision 的官方 Python scorer 为最终真相。
- **清理入口**：当前 CLI 尚未提供 purge 子命令；私有预测和运行产物会持续保留，直到操作者显式删除对应 run 目录。
- **安全诊断**：`benchmark status`、`score.json` 和 `report.md` 提供按状态、失败类别、固定失败原因、工具调用/失败数及累计耗时聚合的 `diagnostics`。报告把失败归到 `agent`、`model`、`gaia_integration` 或 `agent_or_model_bridge`；最后一类覆盖任务级超时（模型与 Agent 循环无法安全区分）以及只有通用 backend 失败的旧记录，不能安全地进一步细分。新运行只持久化白名单原因码（例如上下文超限、模型协议失败、Agent 工具失败、附件接入失败），不会持久化原始 provider 错误。这些字段不包含题目、答案、工具参数、原始工具名或本地路径。
