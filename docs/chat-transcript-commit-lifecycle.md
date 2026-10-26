# 对话记录提交与引擎回收

> 英文镜像：[`chat-transcript-commit-lifecycle.en.md`](./chat-transcript-commit-lifecycle.en.md)（按 CONTRIBUTING 对新开发者文档的英文要求补充；两文内容一一对应，分歧以中文原文为准）。

应用会在模型热切换、空闲回收、会话删除重建以及失败 spawn 清理等时机回收引擎实例。底座在空闲期间仍可通过 `SessionUpdated` 追加内部运行时消息，例如 MCP 启动失败说明。

此前，事件转发器通过 `spawn_blocking` 写入对话记录，等待写入完成后才发出 `chat:transcript_committed`。引擎回收可以中止等待中的转发器，却不能中止已经开始的 blocking 写入。于是文件已增加运行时消息，界面仍只知道上一个提交版本；严格版本对账一直失败，并误报“权威记录暂未同步”。

修复将普通对话的引擎快照写入，以及终态时乐观准入显示消息的回退写入，连同版本计算和提交通知一起放在同一个 blocking 事务中。写入成功后即使转发器被中止，也会发布这一份快照的版本（消息数只进入后端诊断，前端载荷仅含版本）；失败不发布提交标记。终态缓存继续避免重复保存，后续运行时提交会提供更新的版本，不放宽界面的版本比较或编辑 CAS。

每个引擎实例另有独立的写入存活标记。回收先撤销标记再中止转发器；未发布引擎的销毁在发送有界 Shutdown 之前就撤销标记，因为转发器在该 await 期间仍然存活。写入在 SessionStore 的 mutation 锁内检查标记，排队中的旧引擎写入因而不能覆盖后继任务；被拒绝的准入回退写入由回收方自行重新写入并发布。已获准的写入持有同一把锁直至通知结束；新引擎在 blocking worker 上取得这把锁后读取启动基线，保证基线包含这些已完成的提交。界面查询仍可使用普通只读快照接口。

两个次序注记：其一，回收方自己的兜底落盘（`reclaimed_fallback`）仍受 5 秒持门 join 上界约束，超时转后台落定后可能无标记落盘，且回收方会先发出 `chat:done`（Interrupted），晚到的 `chat:transcript_committed` 落在其后——前端对晚到提交的对账（`terminal_seen_before_event`）会吸收该次序；这是先于本修复就存在的窗口，且窄于本文闭合的 forwarder 窗口。其二，基线读在全局 mutation 门下进行，因此任一会话的持久化（含门内的小型诊断追加与事件发布）会短暂延迟其他会话的冷 spawn——这是换取次序保证的既有代价，界面会话列表读取不经过该门，不受影响。

回归测试使用合成任务和合成 MCP 运行时消息，覆盖写入已完成、通知尚未发布时中止转发器，后继引擎基线的等待顺序、退休写入拒绝、后继任务保全、旧版本与截断覆盖的 CAS 拒绝、准入回退的事务内发布与退休拒绝，以及失败写入不发布通知。源码顺序钉另外固定：转发器的每个保存分支都必须克隆引擎的写入标记，回收与失败 spawn 销毁都先撤销标记，spawn 路径对 `sync_session` 失败对所有会话类型一律失败。测试不需要模型、联网或用户会话内容：

```sh
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib transcript_commit_tests -- --test-threads=1
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib transcript_persist_pins
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib scheduled_model_tests
```
