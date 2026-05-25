# WorkFlow Phase 可视化 · 设计（愿景 / 用户旅程）

> 已落地的实现架构（per-session 绑定、命令、数据流）见 [`WorkFlow-Phase-可视化-实现设计.md`](./WorkFlow-Phase-可视化-实现设计.md)。

## 一句话

把"工作流"视图从 占位 升级为 **带当前 skill 的 phase pipeline 状态显示的会话工作区**——用户能一眼看到 LLM 正在 skill-phase 大型工作流的哪一步。

## 背景

- pinvou3 有"工作流"侧边视图，但一直只是 `📋 功能开发中` 占位卡片。
- 与此同时，h3c-ppt 这类大型 skill 已经在 SKILL.md frontmatter 里声明了 `phases: p0:五问|p1:收料|p2:调研*|...` 16 步流程定义。

## 目标

- pinvou3用户，能够通过选择品悟工作流（实际上对应的是一个skill），能够清晰的知道当前的skill是做什么的：目标是什么、产物是什么、产物demo渲染、有多少个步骤（phase）、当前处于哪一个步骤了
- 工作流（skill）通过动态话工作流渲染的方式引导用户可视化的更好的完成某个工作。
- 用户也能够将平时的工作，通过类似claude的/skill-creator，去创建品悟专属工作流（skill）

## 方案

### 用户旅程

1. **打开 app**，左侧切到「工作流」视图。
2. **工作区界面选 skill**，选择skill后，视图上方渲染该skill 的全部 phase chips 横向排列：`[p0:五问] › [p1:收料] › [p2:调研↻] › ...`
   - 全部初态为灰色"待执行"。 视图下方渲染工作会话。
3. **在工作会话**，让 LLM 做一个 h3c-ppt 任务（"帮我做一份 XX 客户的方案 PPT"）。
4. llm应该能知道当前处于那个phase,马上进入那个phase。
5. **查看工作流视图**：
   - 当前 phase chip 蓝色 + ▶ + 脉冲动画。
   - 已走过的 phase chip 绿色 + ✓。
   - 未到的 phase 灰色。
   - loop phase 带 ↻ 标识。
6. 流程跑完，整条 pipeline 大半绿。
    
