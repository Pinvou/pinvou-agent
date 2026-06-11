# 04 · SubAgent（数据平面）

> 架构层文档 · 通用 · SubAgent 是 SDAN 的"干活的腿"

## 定位

数据平面的成员，**傻腿**：收一个 Task → 干活 → 交一个 Result。不知道全局、不参与调度。它的"聪明"只用在**把自己这摊活干好**，不用在"现在该轮到谁"。

## 契约（铁律）

1. **对外只连 Router** —— 不跟别的 agent、不跟用户、不跟裁决直接说话。
2. **只交 Result，且只有两种**：`completed + 产物` / `failed + 原因`。**没有第三种**（连"需要用户输入"也归到 `failed + reason`，或由收集类自己处理，见下）。
3. **内部重试 / 回退自管** —— 它在自己的 `max_steps` 内怎么折腾（从头还是中间重来），是黑盒，Router 不可见、不可管。
4. **绝不发起跨节点回滚** —— 它没有全局视角，也没这个权。它失败就交"失败 + 原因"，**回滚的决策权 100% 在 Router**（见 `06` 回滚三源）。

## 隔离

SubAgent 在**独立上下文**执行：

- 看不到路由表、看不到别的 agent、看不到全局 State；
- 只看到自己收到的那个 Task（含已打包好的 inputs + 工具白名单）。

隔离的收益：可独立替换、可并行、可复用，且不会因为"看到太多"而行为漂移。

## 工具与约束（都来自 registry）

- **`allowed_tools` 白名单**：决定这个 SubAgent 能干什么（写文件 / 执行 / 检索 / …）。**给哪些 tool＝给哪些能力**。
- **`max_steps` / `timeout`**：执行上界，超了即失败。

## 生命周期

```
Router 派 Task → SubAgent 独立 turn loop 执行（用 allowed_tools）→ 交 Result
   completed → Router 走裁决/解锁下游
   failed    → Router 决定 local 重派（≤max_retries）或 blocked
```

## 收集类 SubAgent（跟用户要信息）

有一类 SubAgent 的职责本身就是**跟用户/外部要信息**（需求澄清、信息或素材收集等）。它们的特殊之处：

- 用 **`request_user_input` 工具 + 自己的卡片 UI** 跟用户问答，要到信息就继续、最后交 Result。
- 这是"工作流 → 要信息"的**节点级**通道：`request_user_input` 直接上浮成用户可见的收集类卡片（见 `09` 用户界面平面）。
- 区别于"用户 → 主动干预"（那条是改控制平面——卡牌 + route_table，无自然语言入口，现阶段推迟，见 `09`）。
- 路由表需标明这类节点（它的 `allowed_tools` 含 `request_user_input`），Router 据此知道它可能跑得久（在等用户）、要配卡片通道。

> 这一设计把"要用户输入"从"任意节点的异常中断"变成"某类节点的正常职责"——`need_input` 这种异常状态因此不存在，协议更干净。

**"等用户"对 Router 透明（④）**：收集类 SubAgent 在卡片上等用户回答，是它**自己肚子里的事**——对 Router 来说它就是"还在跑（`dispatched`）"，**Router 不设 `waiting` 状态、不管超时**。卡太久（用户不回）由 SubAgent 自己的 `timeout`（← registry）兜底：超时即交 `failed`，Router 按失败处理（local 重派 → 最终 blocked + 系统通知卡片，见 `11`#3）。别的并行支照跑——这本就由"事件驱动 + 该节点没交 Result"天然成立，不需要专门的 Router 状态。

## 实现

底座（DeepSeek-TUI）已有 SubAgent 的现成实现可复用：`run_subagent`（独立 turn loop）、custom subagent type、`allowed_tools` 白名单、以及支撑卡片问答的 `request_user_input` 机制。SDAN 不重造，只规定**契约**（上面这些），具体由底座实现承载。
