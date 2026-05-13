# pinvou3 进度记录

跨阶段待办、决策、follow-up 集中地。
决策细节走 git commit + `~/.claude/plans/`，这里只放**需要单独排期**的事。

---

## 下一步规划

### 缩减 baseline tools schema 占用（待排期）

**问题**

deepseek-tui 默认注册 41 个工具，初始 baseline 占 ~28k tokens / 66k 上下文 = **44%**。
普通用户（写文档/总结/翻译/查信息/上传文件）用不到 90% 的 tools，但每轮请求都要把全部
schema 重发给 LLM。

**分析数据（2026-05-13 实测）**

| 组成 | 字节 | 估算 token |
|---|---:|---:|
| DeepSeek-TUI 内嵌 `prompts/base.md` | 21 KB | ~7 k |
| DeepSeek-TUI 内嵌 `base.txt` + `cycle_handoff.md` + `modes/agent.md` | ~11 KB | ~3-4 k |
| pinvou3 自己的 `instructions.md` | 2.8 KB | ~1 k |
| 41 个 tool 的 schema（description + input_schema） | — | ~12-18 k |
| **合计 baseline** | | **~22-28 k** ✅ 匹配实测 |

用户从 ChatRoom 首句"你好"实测：input_tokens = 28k+，进度条 44%。

**目标**

把 baseline 从 28k 降到 20-23k 左右（占比从 44% → 30-35%），让对话能容得下更多轮才需要
压缩。

**可行方案：关闭非必要的上游 Feature**

deepseek-tui 提供 `Features::disable(Feature::X)` API（粒度有限，控制大块工具组）。
pinvou3 用户角色下可关：

| Feature | 是否关 | 原因 |
|---|---|---|
| `ApplyPatch` | ❌ 关 | LLM 直接 patch 代码，非 coder 用户用不到，schema 占用较大（apply_patch tool 自带 47KB 源码） |
| `Subagents` | ❌ 关 | sub-agent 协作，普通对话用不到 |
| `ExecPolicy` | ❌ 关 | 复杂 shell 安全策略，pinvou3 是 trust_mode=true 用不上 |
| `ShellTool` | ✅ 保留 | exec_shell 是常用能力（"帮我跑一下..."） |
| `WebSearch` | ✅ 保留 | web_search / fetch_url 高频 |
| `Mcp` | ✅ 保留 | 默认 mcp.json 为空，开销小，将来接外部 API 时直接用 |

**预计效果**：baseline -5~8k → ~20-23k，占比从 44% 降到 30-35%。

**实施位置**：`pinvou3-app/src-tauri/src/bridge/mod.rs::build_engine_config`。
在 destructure-透传 `features` 字段处插入 3 行 disable 调用，大约 10 行总改动：

```rust
let mut features = features;
features.disable(deepseek_tui::features::Feature::ApplyPatch);
features.disable(deepseek_tui::features::Feature::Subagents);
features.disable(deepseek_tui::features::Feature::ExecPolicy);
```

**为什么暂时不做**

1. 当前 baseline 不影响功能，只是占比偏高，对话仍能跑很多轮才触发压缩
2. 自动 / 手动 compaction 已经能兜底
3. 想观察一段实际使用后再决定要不要动 Feature——避免误关导致后续场景出问题
4. 长远方案是 PR 上游加 `tools_allowlist` API 精确剪（每个 tool 单独 enable/disable），
   能再省 8k+。该 API 不存在，需要 fork 改

**搭配方案（用户侧）**

vLLM 启动参数 `--max-model-len 131072`（Qwen3.6 标称支持 128k）。改后 baseline 28k
占比从 44% → 22%。pinvou3 端不需要改。

**风险与缓解**

- 若用户场景需要某个被关掉的 feature → 在 `~/.pinvou3/settings.json` 加 `advanced.enable_features=["ApplyPatch", ...]` 后门覆盖（实施时一并加）
- features 关错可能让某些 system prompt 段落引用到不存在的 tool → 测试需要跑一遍阶段
  A 的 5 个回归任务

---

## 其他已记录 follow-up

- **#34** 敏感目录硬拦截 hook 注册（当前 instructions.md 软引导已生效，硬拦截需要上游
  in-process hook 注册 API 或在 bridge 层后置过滤）
- **#35** busy 时切换 session 改为「先 cancel → 等 done → 真切换」（当前是禁止+system
  message 提示，体验可接受）

---

## 已完成阶段速览

| 阶段 | 节点 | 简述 |
|---|---|---|
| A | 验证 | Qwen3.6 + DeepSeek-TUI 5/5 验证通过 |
| B | commit `7d9cde1` | Tauri MVP + bridge 抽象层 + 4-view 壳 + 三主题 |
| C | 当前 | 多对话历史 / 产物面板 / 取消重发 / 文件上传 / token 警告条 |
