# PinvouOS：无 Session 的连续多 Agent 运行时（取代 ADR-0006 的产品上层模型）

## 状态

已接受，第一阶段开始落地。ADR-0006 描述的“会话内主动委派”在现有 Pinvou-Agent
产品中仍然有效，但不再是 PinvouOS 的目标产品模型。CodeWhale 的 session、thread、
conversation id 可以在迁移期继续存在于执行适配层，不能进入 PinvouOS 的身份、记忆、
任务、能力或并发协议。

## 第一性事实

1. 用户面对的是一个连续存在的 Pinvou，不是会话列表中的多个助手。
2. 对话只是世界中发生的一类事件，不是智能运行时的容器。
3. 一个模型调用的上下文窗口有边界，不等于 Pinvou 的记忆有边界。
4. 多个 Agent 会并发观察、推理和执行；任一 Agent 形成的新事实都可能改变其他
   Agent 的继续、暂停、取消或重规划决定。
5. 温度、权限、资源配额和取消必须由确定性 Kernel 执行。资源 Agent 负责观察和
   提交 Claim，不能靠一段自然语言直接杀死另一个 Agent。

## 决策

### 永远只有一个 Pinvou Identity

运行时使用稳定身份 `pinvou`。前台交互 Agent 是该身份唯一的用户表达面；后台 Agent
不得各自形成互相冲突的“人格”或直接争抢用户界面。

### 删除 Session 作为领域概念

PinvouOS 的并发与因果关系由以下实体表达：

- **Mission**：用户或系统希望持续达成的目标；
- **Run**：某个 Mission 的一次执行尝试；
- **Agent**：常驻系统职责或临时 Mission 执行者；
- **Event**：不可变、带因果引用的运行事实；
- **Claim**：Agent 根据 Evidence 对世界作出的可撤销结论；
- **Directive**：Kernel/Governor 发出的确定性控制决定。

模型供应商的 thread/session/continuation id 只允许作为可丢弃的执行缓存，由后续
adapter 私有保存。业务代码、前端协议和事件账本不得把它们当主键。

### 能力原子化，但不把“能力”和“Agent”混成同一个东西

原子单位是 **Capability Contract**：一个可验证的输入、输出、前置条件、副作用、
权限、资源级别和中断语义。Agent 是一个或多个 Capability 的执行容器。这样既支持
“一种能力包装成一个专职 Agent”，也避免为了调用一个纯函数就制造一个新的自主人格。

Pinvou 判断“能不能做”时只读当前注册的 Capability Contract、Agent 实际/期望状态、
设备资源压力和权限前置条件；不得让模型凭印象声称自己有能力。

### 共享事件账本，不做 Agent 间自然语言群聊

所有 Agent 通过 append-only Event Ledger 共享结构化事实。事件包含 `event_id`、
`sequence`、`source_actor_id`、`mission_id`、`run_id`、`causation_id` 和
`correlation_id`。运行快照是事件投影，可随时由账本重放恢复，不是第二份真相源。

### 资源治理闭环

常驻 Resource Agent 周期采样 CPU、内存、GPU 温度/利用率和功耗，产生
`ResourceObserved` 与带 Evidence 的 Claim。Governor 根据固定阈值生成 Directive：

- warm：只记录压力，不中断；
- hot：暂停可中断、低优先级的 Mission Agent；
- critical：硬停止 Mission Agent；
- 恢复 normal：恢复此前由 Governor 暂停的 Agent。

Directive 先改变 Agent 的 desired state；执行适配器确认后再改变 observed state，
避免“账本说已经暂停、底层模型其实仍在运行”的假事实。

## 第一阶段边界

本阶段优先实现智能与可观测性，不优化 token：

1. 建立单一 Identity、内置常驻 Agent、Capability Catalog；
2. 建立 Mission/Run 和统一 Event/Claim/Directive 账本；
3. 建立常驻 Resource Agent 与 Governor 闭环；
4. 暴露快照、能力可用性、Mission 创建和 Directive 确认命令；
5. 保留旧 SessionStore/EnginePool 作为尚未迁完的执行兼容层。

本阶段仍必须从第一天记录 token、延迟、能耗、取消和因果链，因为第二阶段的上下文
编译、模型路由、Agent 合并/休眠与缓存优化只能基于真实 trace 进行。

## 后续迁移顺序

1. 给 CodeWhale/ACP 增加私有 Execution Adapter，把 Run 绑定到可丢弃的底层线程；
2. 将用户输入、模型输出、工具调用和产物转写为 PinvouOS Event；
3. 前端改成一个连续的 Pinvou 时间线，以 Mission/Run 过滤而非切换 Session；
4. 迁移记忆、知识挂载和工作区所有权；
5. 删除产品层 Session 命令、存储和 UI，最后再删除兼容 adapter。

## 不做的事

- 不在 App 层复制 CodeWhale 的模型工具循环；
- 不让 Resource Agent 或任意 LLM 自己执行硬资源策略；
- 不把整段 Agent 对话互相转发；
- 不通过给现有 `session_id` 改名来假装完成迁移。
