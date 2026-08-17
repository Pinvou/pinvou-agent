# GAIA 官方 Adapter 设计

**状态：** 已确认设计，待实施计划

**首批范围：** GAIA 2023 Level 1 validation，pass@1

**目标：** 让 `pinvou benchmark` 使用 GAIA 官方数据、真实附件和官方 scorer 语义评测完整 Pinvou Agent；不把 Smoke、抽样结果或私有 test 推测包装成官方成绩。

## 1. 官方基线

- 数据仓库：`gaia-benchmark/GAIA`，固定 revision `682dd723ee1e1697e00360edccf2366dc8418dd9`。
- 首批配置：`2023_level1`，split=`validation`。
- 数据格式：Parquet；读取字段固定为 `task_id`、`Question`、`Level`、`Final answer`、`file_name`、`file_path` 和 `Annotator Metadata`。
- scorer 来源：`gaia-benchmark/leaderboard/scorer.py`，固定 revision `1349a17979f0aca0ee9c46cd7ec26eb2fb41102e`。
- validation 的 `Final answer` 可用于本地评分；test 的答案和部分元数据是私有的，本地只能生成 submission，不能生成或宣称 test 分数。
- GAIA gated 数据不得复制到仓库、日志、公开报告或可抓取位置。仓库只保存 revision、schema、哈希清单和不含题目/答案的汇总。

## 2. 模块边界

新增独立 `pinvou-cli/crates/adapter-gaia`。该 crate 只依赖 `benchmark-core` 和解析/哈希库，不依赖 Tauri、EnginePool 或 GUI。

职责拆分：

1. `dataset`：读取固定 Parquet schema、解析 task、校验 Level/split/revision、解析相对附件路径。
2. `fetch`：从 Hugging Face gated dataset 下载固定 snapshot，或导入用户指定的已有 snapshot。
3. `verify`：离线验证 revision marker、Parquet、附件存在性、路径 containment、文件哈希与任务 ID 唯一性。
4. `adapter`：实现 `BenchmarkAdapter`，把问题和附件转换为 `NativeTurn`，声明 durable UTF-8 private prediction 和 `pinvou-gaia-public-web/v1`。
5. `scorer`：兼容固定官方 `question_scorer` 语义，产生逐题 0/1 和聚合准确率。
6. `submission`：输出官方要求的 JSONL；只在用户显式指定目标路径时导出私有预测。

`benchmark-core` 不感知 GAIA 字段；`pinvou-product-backend` 不感知 GAIA scorer。CLI 只注册 adapter 和路由命令。

## 3. 获取与验证

支持两个等价入口：

```text
pinvou benchmark fetch gaia --token-env HF_TOKEN
pinvou benchmark fetch gaia --source <snapshot-directory>
```

- 自动下载只从 `HF_TOKEN` 指定的环境变量读取 token，不接受命令行明文 token，不打印 token，不保存 token。
- `--source` 不复制任意目录内容到报告；只导入预期 revision 的 GAIA snapshot，并生成本地私有索引。
- 下载/导入目标放在用户运行时数据目录，不进入 Git 工作树。
- fetch 完成后必须自动执行 verify。校验失败不产生“可运行”数据集状态。
- revision 不匹配、gated access 未授权、schema 漂移、附件越界/缺失、重复 task ID 均使用固定安全错误码。
- 初版不自动跟随 `main`；升级 revision 必须修改 adapter 常量、golden 和迁移说明。

## 4. 任务执行

每个官方 row 映射为一个 `BenchmarkTask`：

- `task_id` 原样作为受信 suite ID，但写报告前仍走通用安全 ID 检查。
- `Question` 只通过 private input resolver 进入产品 runtime，不写 manifest/events/report。
- `file_path` 必须是 snapshot root 下的直接或嵌套 regular file；解析后以 opaque attachment handle 交给通用附件 resolver。
- 所有任务使用同一个运行开始时固定的产品模型快照。
- `tool_policy_id` 固定为 `pinvou-gaia-public-web/v1`；不允许 adapter 或 CLI 提供任意工具数组。
- concurrency=1、pass@1；单题失败/超时要持久化安全终态并继续其他题。
- 答案以 durable private prediction 保存，公开 JSONL 只保留 core 生成的 opaque handle。
- resume 必须校验 dataset/scorer/adapter revision、split、模型、tool policy、pass 和 concurrency；任一不一致固定拒绝。

## 5. 官方 scorer 兼容性

Rust scorer 必须用官方 scorer 的 golden fixture 做逐分支等价验证：

- 数字答案：移除 `$`、`%`、`,` 后转 float，并与 ground truth float 精确比较。
- 列表答案：ground truth 含 `,` 或 `;` 时按两者切分，元素数量必须相同；数字元素走数字规则，字符串元素移除空白、转小写，但保留标点。
- 普通字符串：移除全部空白、ASCII punctuation、转小写后比较。
- candidate 缺失按官方当前语义视为字符串 `None`。

本地 scorer 版本写入 manifest 和 score report。任何无法证明等价的变化必须升级 scorer revision/version；不得“改良”官方 scorer 后仍标为官方兼容。

## 6. 成绩与诚实标识

validation 完整 Level 1 跑完并使用锁定 scorer 时，输出：

- `official_dataset_compatible=true`
- dataset/scorer revision
- split、level、pass、总题数、完成/失败/超时数
- 正确题数和 accuracy
- 模型 identity、tool policy、运行时间和绝对报告路径

报告标题必须为“GAIA 2023 Level 1 validation — 本地官方兼容复现”。不得写“官方排行榜成绩”。validation 已公开且存在污染风险，因此只用于开发回归和内部同配置比较。

以下情况只输出 `unofficial/partial`，不生成可比较准确率：过滤题目、抽样、缺题、scorer revision 不匹配、附件缺失、非 pass@1、模型混跑或运行未完成。

test split：不读取/推断 ground truth；只生成 submission 和完成度报告，显示 `local_scoring_unavailable`。只有官方 leaderboard 返回的结果才能标记为 test 官方成绩。

## 7. Submission

```text
pinvou benchmark submission gaia --run <run-id> --output <path>
```

- 只接受完整且 manifest 匹配的 GAIA run。
- 从 run-bound `ScorerView` 解析 durable private prediction；公开 handle 本身不能解析答案。
- 使用 `create_new`/原子发布，拒绝覆盖、symlink/reparse 和目录逃逸；输出文件使用用户私有权限。
- submission 是唯一允许把候选答案导出私有 store 的路径。
- validation/test submission 均记录 dataset revision、task coverage 和格式版本，但不包含 prompt、附件内容、tool I/O、session ID 或原始错误。

## 8. CLI

首批开放：

```text
pinvou benchmark fetch gaia ...
pinvou benchmark verify gaia ...
pinvou benchmark run gaia --split validation --level 1
pinvou benchmark status <run-id>
pinvou benchmark resume <run-id>
pinvou benchmark score gaia --run <run-id>
pinvou benchmark report <run-id>
pinvou benchmark submission gaia --run <run-id> --output <path>
```

`benchmark list` 将 GAIA 从 `planned` 改为 `available`，但只声明 `validation level1`；Level 2/3 与 test execution 在实现和验证前仍显式 unavailable。

退出码沿用通用 CLI：成功 0、运行/评分失败 1、参数/不可用能力 2。human/json 输出均使用固定错误码，不打印底层路径、HTTP body、token 或答案。

## 9. 测试与验收

不把官方 gated 数据提交为 fixture。测试分三层：

1. 合成 schema fixtures：Parquet 字段、路径逃逸、重复 ID、缺附件、revision mismatch。
2. scorer golden：从官方 scorer 各语义分支提取不含 GAIA 题目的自建输入，逐项验证 Rust 结果。
3. 用户本地 gated snapshot 契约：只检查任务数、字段、附件覆盖和 aggregate，不记录题目/答案。

首批完成门禁：

- `adapter-gaia` package tests 全绿。
- fetch/import 后 verify 全绿，且仓库/报告隐私扫描无题目、答案、token、原始路径。
- 使用真实 Pinvou Agent 完整运行 Level 1 validation，失败任务不中止整批。
- run 进程退出后重新打开并评分成功，证明 durable private prediction/resume 生效。
- Rust scorer 与锁定官方 scorer 对同一批本地 validation candidate 得到逐题一致结果。
- submission task coverage、字段和顺序通过官方格式校验。
- 普通 GUI 和 Smoke 回归不走 GAIA policy。

## 10. 非目标与后续

首批不实现 Level 2/3、GUI benchmark UI、自动上传 leaderboard、绕过 Hugging Face gating、test 本地评分、并行多模型或 Judge 评分。Level 1 validation 完整闭环后，再以同一 adapter 扩展 Level 2/3；test submission 上传需单独的外部副作用确认。
