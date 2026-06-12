看完了 PR #7(xuyajing → main,+6711/−1946,25 文件)。这是一个**「工具市场 + iwencai 接入」**的大 PR,核心接线我都过了一遍。结论:功能方向 OK,但现在不能合,有阻断性问题。

🔴 阻断级 —— 分支太旧,会复活已删代码

PR 落后 main 近 2000 个 commit、merge-base 停在 c05b281。后果:

- review_gate 整套品悟 v2 通路在 PR 分支里还完整存在(review_gate.rs + review_gate_test_l2…l16 十几个模块),而 main 早已剥除。一旦合并 = 死代码复活 + 大面积冲突。
- tests/pinvou_ab_test.rs(新增 835 行) 直接 use ...review_gate::check_exit_gate / mode_state::PlanPhase —— 正是 main 已删的通路。rebase 到当前 main 后编译失败。这跟你之前记的「v3 接线依赖已删 v2 通路 = 合并地雷」是同一个坑。
- 而且这个 A/B 测试和 PR 主题(iwencai/工具市场)毫无关系,是夹带进来的。同理还有一批夹带 docs(架构文档/字号规范/产物导出方案 等 ~2400 行)。

👉 必须先让作者把分支 rebase 到最新 main,解掉 review_gate 复活,剔除无关的 ab_test 和夹带文档,PR 才有得审。当前状态是 CONFLICTING/DIRTY。

🟠 功能 bug(即使解决分支问题也要修)

1. command: python 会让 Linux 客户机的 present server 起不来。ensure_builtin_mcp_servers() 每次启动用 "command": "python" 覆写 pinvou 条目(bundle.rs:284),但 DEFAULT_MCP_JSON 原本是 python3。weather/iwencai/tdx 三个 manifest 也全是 python。很多 Linux/deb 环境只有 python3 没有 python 软链 → present_artifact 卡片直接挂。作者大概为 Windows 改的,但不该牺牲 Linux,要按平台分支(Win=python,Unix=python3)。
2. pip 运行时懒装与既定策略冲突。install_pip_deps 用裸 pip install(无 --user/venv),deb 环境会撞 PEP 668 externally-managed 直接失败。你之前定的是「外部依赖走 deb Depends/Recommends 声明,不做运行时懒装」—— 这个 PR 走了反方向。
3. tdx 接线缺失:resources/mcp-servers/tdx/ 进了仓库,但 bundle.rs 只 include_str! 了 weather/iwencai,没内嵌 tdx → 市场里扫不到,装了也没文件。要么补全要么先别进。
4. weather/__pycache__/server.cpython-312.pyc 被提交进 git,且 .gitignore 没有 __pycache__ 规则。删文件 + 补 gitignore。

🟢 看着合理的部分

- marketplace.rs 整体设计干净:manifest 扫描 / installed.json 持久化 / mcp.json upsert-merge,没有重复造底座轮子,符合「Platform 只做编排」边界。
- mcp.json 从「VERSION-gated 全量覆写」改成「每次启动 merge upsert pinvou、保留 marketplace 条目」—— 解决「升级覆盖已装工具」的思路是对的。
- instructions.md 的 {{MARKETPLACE_TOOLS}} 占位符:因为 BUNDLE_VERSION 内嵌了 instructions.md 哈希,改动会自动 bump 版本触发重写落盘,这块没问题。
- open_external_url 加 iwencai 白名单且同步改了测试注释,守住了 XSS 防线的约定。