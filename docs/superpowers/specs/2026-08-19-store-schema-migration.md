# Store / Spool / WAL Schema 迁移与降级策略

> 状态：草案（draft）。日期：2026-08-19。
> 上游：蓝图 §21（协议版本化）、§24.5（更新与回滚："三 binary 一起回滚，不留混合版本"）。本文档补齐蓝图缺失的**数据**兼容层：binary 一起回滚 ≠ 数据一起回滚。

## 1. 问题

蓝图 §24.5 说升级失败时三个 binary 一起回滚。但新版 daemon 可能已经用新 schema 写过 Store/spool/WAL——旧版 binary 面对新版数据的正确行为是什么？蓝图没有回答。本文档定义三层版本与降级规则。

## 2. 三层版本模型

| 层 | 载体 | 版本位置 | 演进节奏 |
|---|---|---|---|
| L1 协议 | IPC 帧 / Node Protocol | 消息信封 `v` + 握手协商（蓝图 §21） | 阶段性 |
| L2 存储格式 | spool 段文件 / WAL 段文件 / SQLite | 段头 magic + `format_version`；SQLite 用 `PRAGMA user_version` | 随版本发布 |
| L3 事件 schema | RuntimeEventEnvelope | `schema_version`（事件 schema v1 §5） | 只增不改 |

三层独立演进；一次发布可以只动其中一层。

## 3. 规则

### 3.1 spool / WAL 段文件

- 段头：`magic + format_version + segment_id + stream/attachment 元数据`。每条记录必须包含 `record_len + record_version + payload + CRC`，否则 reader 无法安全跳过未知扩展。
- 读取方遇到 **更高** `format_version`：拒绝读取该段，显式错误（退出码 8 的数据路径）+ 指明段路径与期望版本。**禁止**按猜测解析（蓝图 §19"Node spool 损坏不得伪造完整历史"的推广）。
- 新版本 reader 必须声明并测试可读取的历史 `format_version` 范围。旧 reader 遇到更高段 `format_version` 一律拒绝；只有段格式版本未变化、记录 framing 明确允许跳过某个可选扩展时，旧 reader 才可忽略该记录的未知尾部字节。
- 段文件校验：每记录 CRC32；段尾 summary 记录条数与字节范围，恢复时校验（蓝图 §12.7 Controller WAL 恢复的数据完整性前提）。

### 3.2 SQLite（Controller/Node Store）

- `user_version` 单调递增；迁移表 `schema_migrations(version, applied_at, checksum)`；binary 同时声明 `min_readable_user_version` 与 `max_readable_user_version`，不得只比较“是否更高”。
- **只允许前向迁移**；迁移在 daemon 启动、持有单实例锁之后、开放 IPC 之前执行。每个迁移必须在 SQLite 事务内完成，迁移数据与 `schema_migrations` 记录原子提交；kill -9 后要么整步回滚，要么整步完成。
- 迁移前使用 SQLite Online Backup API（或关闭全部连接并完成 WAL checkpoint 后复制）生成一致快照，不能在活动 WAL 连接上直接复制单个数据库文件。
- 快照写入数据根下 `backups/pre-v<N>-<timestamp>/`，默认保留最近 3 份。迁移前先计算完整快照空间；若 512MB 默认预算不足且无法安全扩容，必须停止迁移并提示用户，不能删除唯一可回滚快照后继续。
- Additive-only：新增列必须带默认值；禁止删列、改列语义、重命名（重命名 = 加新列 + 迁移数据 + 旧列停写，三个版本后再物理清除）。
- 未知字段（条件式前向兼容）：rusqlite 读取按列名时可以忽略多余列，但只有发布清单声明 rollback-compatible 且旧 binary 的查询/写入语义已通过合同测试时，才能据此允许旧版本打开新写入的数据。

### 3.3 事件 schema（L3）

- 事件 schema v1 §5 已定义（只增 kind、可选字段，并通过握手协商支持范围）。未知安全/控制事件 fail closed；只有确认安全的通知才可保留为 `vendor`。存储层不得因为 L3 版本升级重写历史事件——**事件是不可变历史**，schema 演进只影响投影逻辑。
- 协商使用版本范围 + required feature bits，而不是盲目选择最低共同版本；若低版本缺少审批、取消、独立 control ACK 等必需语义，连接必须显式失败。
- ViewModel/event cursor 可能因压缩或 schema/filter 变化失效。服务端返回结构化 `cursor_expired` 和原子 snapshot version；客户端丢弃旧局部投影，加载快照后从该 version 继续订阅，不能无限保留历史来维持所有 cursor。

## 4. 升级 / 降级 / 回滚矩阵

| 场景 | 行为 |
|---|---|
| 正常升级（次版本） | 启动迁移（前向、additive）；新 binary 读旧数据 = 必须支持 |
| 正常升级（主版本） | 同上，但允许 L2 `format_version` 跳变；跳变前强制生成快照 |
| 升级后回滚（次版本） | 只有发布清单声明“未提高 L2 format/user version，且旧 binary 兼容新写入语义”时才允许直接启动；additive schema 本身不足以证明可回滚。否则按主版本回滚路径恢复快照或先导出 |
| 升级后回滚（主版本） | 旧 binary 遇到更高 `format_version` → **拒绝启动**并输出恢复指引：① 用快照目录恢复；② 或重装新版导出（`pinvou data export`）后回滚。不静默损坏 |
| 双版本并存（用户手动装两份） | 单实例锁（蓝图 §13.1）使第二 daemon 无法打开同一数据根；错误信息包含两进程路径与版本 |
| `pinvou data export` | 不进入阶段 1 Walking Skeleton；在首次需要不可直接回滚的 L2 变更前提供数据根打包（tar/zip）+ manifest（版本、checksum、时间），并同时冻结 import/恢复流程 |

## 5. 合同测试清单（进 CI T0/T1）

1. 段文件版本拒读：伪造高 `format_version` 段 → reader 返回显式错误、不 panic。
2. 未知尾部字节：保持相同 `format_version`，用带 `record_len/record_version` 的新 writer 写可跳过扩展 → 旧 reader 完整解析已知字段并安全跳过扩展；提高段 `format_version` 时旧 reader 必须拒读。
3. SQLite 前向迁移幂等：同一迁移跑两次 → 第二次 no-op。
4. 迁移中断恢复：迁移事务中途 kill -9 → 重启后该迁移处于完整未应用或完整已应用状态，不允许半迁移；快照生成幂等。
5. **回滚可读性（核心）**：仅对发布清单标记 rollback-compatible 的次版本执行“新版本写入 → 上一发布 tag 启动并完整读取”；未标记兼容的版本必须拒绝直接启动并验证快照恢复指引。
6. CRC 损坏检测：篡改段内字节 → 恢复路径报告缺口（蓝图 §19"报告明确事件缺口，不伪造完整历史"）。
7. 快照空间不足：数据库大于默认备份预算 → 迁移停止、原库不变、既有最后可用快照不被误删。
8. L3 双向 golden：N-1 writer → N reader 与 N writer → N-1 reader；未知字段的二进制 round-trip 不丢失，JSON/vendor extension 的保留或拒绝策略显式断言。
9. cursor 压缩恢复：有效、已压缩、伪造、其他 filter/schema 四类 cursor；`snapshot + resubscribe` 之间无丢事件窗口且不重复投影。

## 6. 对蓝图的回写项

- §21 增补：存储格式版本（L2）与协议版本（L1）分节，引用本文档。
- §24.5"失败时三个 binary 一起回滚"增补一句："binary 回滚的数据兼容规则见 `2026-08-19-store-schema-migration.md` §4 矩阵"。
- §26 增补：主版本升级的 export 强制策略作为待冻结参数（阶段 1 不需要，首个主版本前冻结）。
