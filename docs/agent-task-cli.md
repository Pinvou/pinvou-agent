# 无头 agentic 单任务 CLI(`pinvou agent run`)

`pinvou agent run` 在窗口化 Tauri 宿主里执行**一次产品等价的 agentic 轮次**:与 GUI 聊天同一条发送链路(Yolo 模式、产品工具白名单、Bash/File(write/edit)/Git/Web 真实执行),会话执行根绑定到调用方指定的任务目录。它面向外部评测 harness(如 Terminal-Bench/Harbor)把 Pinvou Agent 装进任务容器执行真实终端工作的场景,不经过评测后端(`HeadlessAgentBackend`),也不触碰任何 eval 工具策略——GAIA 等只读评测的隔离语义不受影响。

与 `pinvou benchmark ...` 的关系:benchmark 子命令族面向固定数据集的批量评测,带只读工具策略与隐私输出管线;`agent run` 面向单条任务指令的完整 agentic 执行,输出(assistant 文本、工具事件、usage)直接返回给调用方。两者共用同一 headless 宿主引导。

## 构建与运行

```bash
cargo build --manifest-path pinvou-cli/Cargo.toml --bin pinvou
PINVOU3_HOME=/path/to/sandbox pinvou agent run \
    --prompt-file task.txt \
    --workspace /path/to/task-dir \
    --timeout-secs 600 \
    --output json
```

- `--prompt-file`(必填):任务指令文件,内容原样进入产品发送链路,不加评测信封。
- `--workspace`(可选):任务工作目录;省略时使用会话私有目录(与评测会话一致的隔离 scratch)。提供时,该目录成为引擎 cwd 与 shell 执行目录(机制同原生代码会话的项目绑定,`ExecutionRootResolver` 只对本次生成的会话生效)。
- `--timeout-secs`(默认 600,上限 604800=7 天):轮次超时;超时先走取消,取消后若轮次在沉淀窗口(30s)内仍未结束则直接产出 `timeout` 报告,绝不无限等待。上限在解析期强制——无上限的 u64 会让内部 `Instant + Duration` 溢出 panic。
- `--output human|json`:human 打印会话/状态行加 assistant 文本;json 打印完整报告单行 JSON。

前置条件:沙箱 `PINVOU3_HOME` 的 `settings.json` 需配置激活模型(任意 OpenAI 兼容端点均可,`preset = "openai_compatible"`);`PINVOU3_ALLOW_SHELL=1` 可钉死 shell 授权(不依赖 prefs)。注意 benchmark-hooks 构建默认把每轮工具调用上限钉在 8 次(GAIA 防失控护栏),长程 agentic 任务需 `PINVOU3_MAX_TOOL_CALLS=512` 之类的显式抬高。

## 输出契约

json 报告字段:`session_id`、`status`(`Completed`/`Failed`/`timeout`/`error` 或引擎状态)、`timed_out`、`completed_after_deadline`(超时竞态标记:截止后、取消生效前自然完成的回合保留引擎真实 `status` 而非改写为 `timeout`,判分端可据此区分"真做完但过线"与"被取消";旧报告缺省为 false)、`assistant_text`(最后一轮助手文本)、`tool_events`(仅工具名与成败,不含参数/结果)、`usage`(输入/输出/缓存/思考 token)、`error`(读取轮次结果失败等宿主侧根因,`status=error` 时非空)。工具事件刻意不携带负载,报告可安全落盘到 `/logs` 供 harness 做 usage 汇总。

退出码:只要产出报告(含 `timeout`/`error` 状态)即为 0——轮内失败由 harness 判分器按报告结算,非零退出会让超时任务被记为 exception 而非 0 分,扭曲均值;非零退出码保留给宿主级失败(`--prompt-file` 不可读、后端不可用等),参数错误(缺少 `--prompt-file`、`--timeout-secs` 越界等)为 2。

## 容器内运行(Terminal-Bench 形态)

二进制是动态链接的 Tauri 程序,任务容器需要 GTK3/WebKit2GTK 4.1 运行库与 xvfb(无显示环境下窗口化事件循环仍需 X server):

```dockerfile
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y \
    libwebkit2gtk-4.1-0 libgtk-3-0 libjavascriptcoregtk-4.1-0 \
    libayatana-appindicator3-1 xvfb xauth ca-certificates procps curl
```

运行:`xvfb-run -a pinvou agent run ...`。模型端点需要从容器内可达(docker bridge gateway,如 `http://172.17.0.1:13000/v1`)。注意 glibc 前向兼容:在发行版老基线(如 Debian bookworm)上编译出的二进制可运行于更新的基线,反之不行。

## 实现位置

- `pinvou3-app/src-tauri/src/features/assistant/product_runtime/agentic_task.rs`:宿主引导、执行根绑定、轮次驱动与超时看护。
- `pinvou-cli/crates/pinvou-product-backend`:对外的 `run_agentic_task` 启动器。
- `pinvou-cli/crates/cli/src/lib.rs`:`agent run` 参数解析与输出渲染。
