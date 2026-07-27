# Pinvou 设计模式方案

## 交接状态（给下一个 Codex）

### 当前工作区

- 仓库 worktree：`C:\Users\123\pinvou3-design-mode`
- 前端目录：`C:\Users\123\pinvou3-design-mode\pinvou3-app`
- 分支：`work/design-mode-20260724`
- 基准：`origin/main`，创建时最新提交为 `c4293577 ci: 去重 PR 检查并收敛测试环境锁 (#234)`
- 桌面方案文件：`C:\Users\123\Desktop\pinvou-design-mode方案.md`

### 已完成内容

已完成三步：

1. 工作 / 设计 / 代码三模式入口和状态机。
2. 设计模式 runtime 注入闭环：进入设计模式后，HTML 产物 iframe 可 hover 高亮、click 选中元素，并把 snapshot 回传 Pinvou。
3. 最小设计面板：支持临时修改文案、文字颜色、背景颜色、字号、圆角，并记录 changes log；支持清空修改恢复 iframe 原始状态。

当前实现只改预览，不改源码；还没有做“应用到代码”。

### 关键代码文件

- `pinvou3-app/src/features/chat/ChatView.jsx`
  - 三模式入口：`工作 / 设计 / 代码`
  - 设计模式状态条
  - 最小设计面板 `DesignMiniPanel`
  - changes log 状态
  - 代码模式 provider 占位：Codex / Claude Code / Cursor / 自定义 MCP
- `pinvou3-app/src/features/chat/pinvou-mode-state.js`
  - `work | design | code` 状态模型
  - code provider 持久化
- `pinvou3-app/src/features/chat/design-changes.js`
  - design change 创建、状态更新、去重
- `pinvou3-app/src/features/artifacts/design-runtime.js`
  - iframe 内注入脚本
  - hover 高亮、click 选中
  - apply text/style patch
  - clear changes 恢复原始状态
- `pinvou3-app/src/features/artifacts/ArtifactsPanel.jsx`
  - 给 HTML/Office HTML iframe 注入 runtime
  - 转发 `designCommand`
  - 接收 runtime message
- `pinvou3-app/src/features/settings/SettingsView.jsx`
  - `ScaledHtmlPreview` 增加 `onFrameLoad` 和 iframe `data-testid`
- `pinvou3-app/src-tauri/config/dev-port-1421.conf.json`
  - 临时 dev overlay：把 Tauri `devUrl` 指到 `http://127.0.0.1:1421`

### 新增测试

- `pinvou3-app/tests/pinvou_mode_state.test.js`
- `pinvou3-app/tests/design_runtime_logic.test.js`
- `pinvou3-app/tests/design_changes_logic.test.js`
- `pinvou3-app/tests/design_mode_entry_smoke.js`

### 已验证命令

以下命令在 `C:\Users\123\pinvou3-design-mode\pinvou3-app` 下已跑通：

```powershell
npm run test:pinvou-mode-state
npm run test:design-runtime
npm run test:design-changes
npm run test:design-mode-entry-smoke
npm run build:ui
npm run lint:ui
npm run test:composer-tools
npm run test:composer-tools-smoke
npm run test:web-access-contract
node tests/markdown_artifact_edit_smoke.js
```

构建会出现项目既有 Vite warning：外链 script 不能被 bundle、chunk size 超过 500KB。不是本次改动引入的失败。

### 启动 app 注意事项

用户要求启动桌面 app 时，之前尝试过：

```powershell
git submodule update --init DeepSeek-TUI
$env:PINVOU3_UI_DEV_PORT="1421"
npm run dev -- --config src-tauri/config/dev-port-1421.conf.json
```

第一次启动失败过一次，原因是 `DeepSeek-TUI` 子模块没有初始化。已执行：

```powershell
git submodule update --init DeepSeek-TUI
```

之后前端 Vite 服务能启动到：

```text
http://127.0.0.1:1421
```

但桌面 Tauri app 没等到窗口，因为 Rust 首次编译很慢，长时间停在 `cargo/rustc` 编译 `DeepSeek-TUI`。用户后来要求停掉，相关 `1421`、`cargo`、`rustc` 进程已经停止。

如果下次继续启动，建议隔离 Rust target，避免多个 worktree 共用 `C:\pinwu-cargo-target` 抢锁：

```powershell
$env:PINVOU3_UI_DEV_PORT="1421"
$env:CARGO_TARGET_DIR="C:\pinwu-cargo-target-design-mode"
npm run dev -- --config src-tauri/config/dev-port-1421.conf.json
```

注意：另一个 worktree 可能占用 `1420`，不要直接用默认 `npm run dev`，除非确认 `1420` 空闲。

### 当前已知问题

- 设计模式只支持 HTML / Office HTML iframe 预览；Markdown、图片、PDF 等产物不注入设计 runtime。
- 设计修改只是 runtime 临时 inline style / text patch，不会写源码。
- selector 目前是基础实现：`id`、class、`nth-of-type`，后续需要更稳的 source/component 定位。
- changes log 目前只存在前端状态里，未持久化到会话，也未导出给 Agent。
- 颜色 input 在自动化里需要 native setter；真实用户操作正常。
- `dev-port-1421.conf.json` 是为本地多 worktree 启动加的 dev overlay，PR 前可以保留或视团队规范移除。

### 下一步建议

第四步做“应用到代码”占位闭环：

1. 在设计面板加「应用到代码」按钮。
2. 把当前 changes log 转成结构化任务文案。
3. 如果代码模式 provider 已选择，payload 带上 `provider`。
4. 如果还没有真正 Code Mode executor，先回流到当前 Work / Chat 修改链路。
5. 不要直接由 Design Mode 写源码。

建议新增测试：

- changes log serializer 单测。
- provider 选择后 payload 包含 `codex` / `claude-code` 等 provider。
- browser smoke：设计模式改 h1 -> 点击应用到代码 -> textarea 或任务入口得到结构化修改说明。

## 目标

Pinvou 后续形成三个模式：

- 工作模式：负责需求梳理、任务拆解、页面/功能规划。
- 设计模式：负责对产物预览进行可视化编辑，把用户的视觉意图结构化。
- 代码模式：负责把设计模式产生的变更真正落到源码，并验证产物。

设计模式的定位不是完整替代 Figma，而是在 Pinvou 产物已经能运行后，提供类似 TRAE 的「现场编辑」能力：用户直接在预览页面里点选元素、调整样式、修改文案、留下设计反馈，然后一键交给代码模式改代码。

## 推荐开源底座

### 主参考：SandeepBaskaran/design-mode

地址：https://github.com/SandeepBaskaran/design-mode

选择理由：

- 最接近 Pinvou 想要的设计模式：在 live website 上直接选中元素并编辑。
- 已有 Chrome extension、side panel、visual controls、changes log、tokens、MCP bridge。
- 变更可以通过 MCP 工具暴露给 agent，例如 get_changes、apply_changes、export_changes、get_screenshot。
- MIT license，适合商业产品参考和二次开发。

适合借鉴/复用的模块：

- 页面 overlay 注入。
- 元素选中、高亮、hover、resize handles。
- 视觉编辑面板。
- changes log 数据结构。
- selector 生成和变更导出。
- MCP/WebSocket bridge 思路。

### 辅助参考：joshcirre/instruckt

地址：https://github.com/joshcirre/instruckt

选择理由：

- 更轻，适合学习如何内嵌到 Vite/React/Vue/Svelte 预览环境。
- 有 framework/component/source location 检测思路。
- 支持 annotation、screenshot、structured markdown。
- MIT license。

适合借鉴/复用的模块：

- framework adapter。
- source file hint。
- annotation schema。
- 截图和反馈记录。

### 不建议直接 fork：stagewise

地址：https://github.com/stagewise-io/stagewise

原因：

- 它是完整 Agentic IDE，不只是设计模式。
- 代码和产品边界更重。
- AGPLv3 license 对商业集成有明显约束。

## MVP 范围

第一版设计模式只做 Web 产物预览，不覆盖移动原生、小程序、PPT、海报等非 DOM 产物。

MVP 必须支持：

- 在 Pinvou 预览区域开启/关闭设计模式。
- 鼠标 hover 元素时显示高亮框。
- 点击元素后选中并展示编辑面板。
- 支持修改文案。
- 支持修改颜色、字体大小、字重。
- 支持修改 margin、padding、width、height、border radius。
- 支持对选中元素添加评论。
- 支持保存 changes log。
- 支持应用到代码，把结构化变更交给代码模式。
- 代码模式修改源码后刷新预览。

MVP 暂不做：

- Figma 导入/导出。
- 完整设计系统管理。
- 多人协作。
- 自动生成整套 UI。
- 复杂动画编辑。
- 任意 DOM 重排。
- 非 Web 产物编辑。

## 产品入口设计

第一步应该加设计入口，但入口不能只是一个按钮。它需要和 Pinvou 的模式体系绑定。

推荐入口：

1. 顶部模式切换：工作 / 设计 / 代码 三段式。
2. 预览区域右上角再放一个设计模式开关，作为快捷入口。
3. 输入框上方可以放「当前模式」提示和快捷切换，但不建议只把设计入口藏在输入框上方。

原因：

- 设计模式是一个全局工作模式，不是一次聊天命令。
- 用户进入设计模式后，主要操作对象会从输入框切到产物预览。
- 入口需要改变预览区行为：启用 hover、选中、overlay、编辑面板。

建议第一阶段入口行为：

- 点击「设计」后，Pinvou 切换到设计模式。
- 自动打开当前产物预览。
- 向预览 iframe/WebView 注入 design overlay runtime。
- 输入框 placeholder 改成面向设计反馈，例如「描述你想怎么调整选中的元素」。
- 右侧/侧边出现设计面板。
- 用户选中元素后，输入框可以基于该元素继续下指令。

## 技术架构

核心链路：

```text
Pinvou Shell
  -> Preview iframe/WebView
  -> Design Overlay Runtime
  -> Change Store
  -> 应用到代码
  -> 代码模式 Agent
  -> Source Files
  -> Dev Server Refresh
  -> Visual Verification
```

模块拆分：

- design-runtime：注入到产物页面，负责 hover、select、style override、text edit、comment、screenshot。
- design-panel：Pinvou UI 里的编辑面板，展示选中元素和 controls。
- design-store：记录每次修改，包括 selector、oldValue、newValue、property、comment、screenshot。
- design-bridge：Pinvou shell 和 preview runtime 通信，可以先用 postMessage，后续再升级 MCP/IPC。
- code-handoff：把 changes log 转成代码模式 agent 可执行任务。
- verifier：刷新预览并截图检查。

## 第一阶段实现步骤

### Step 1：加设计模式入口和状态机

新增全局 mode：

```ts
type PinvouMode = "work" | "code" | "design";
```

入口建议放在顶部模式切换区，并同步影响输入框和预览区。

验收标准：

- 用户可以在工作 / 设计 / 代码三个模式之间切换。
- 设计模式开启后，预览区域进入可选中状态。
- 输入框和侧边面板能知道当前处于设计模式。

### Step 2：注入 overlay runtime

在设计模式开启时向预览 iframe/WebView 注入脚本。

验收标准：

- hover 元素有高亮框。
- click 元素后能返回 selector、tagName、className、text、computedStyle。
- 退出设计模式后 overlay 清理干净。

### Step 3：实现最小编辑面板

支持文案、颜色、字号、间距、圆角。

验收标准：

- 修改后页面即时变化。
- 每次变更记录到 changes log。
- 支持撤销单条变更。

### Step 4：应用到代码

把 changes log 交给代码模式 agent。

变更 payload 示例：

```json
{
  "mode": "design",
  "target": {
    "selector": ".hero-title",
    "text": "Build faster with Pinvou",
    "computedStyle": {
      "fontSize": "48px",
      "color": "rgb(17, 24, 39)"
    }
  },
  "changes": [
    {
      "property": "font-size",
      "oldValue": "40px",
      "newValue": "48px"
    }
  ],
  "comment": "标题需要更有主视觉冲击力"
}
```

验收标准：

- 代码模式能定位到相关组件/样式文件。
- 修改真实源码。
- 自动刷新预览。
- changes log 标记为 resolved。

## 开发方案

### 开发阶段 0：准备和边界确认

目标：先确认 Pinvou 当前产物预览的技术形态，决定设计模式如何注入。

需要确认：

- Pinvou 预览区是 iframe、WebView、独立浏览器标签页，还是内嵌开发服务器。
- 预览页面和 Pinvou 主界面是否同源。
- 当前产物源码是否有稳定的文件写入和热更新流程。
- 当前聊天输入框、预览区、右侧面板是否已经有统一状态管理。

输出物：

- 当前预览架构说明。
- 设计模式注入方式选择：优先 postMessage + iframe 注入；如果受跨域限制，再考虑本地 companion server 或浏览器扩展形态。

### 开发阶段 1：三模式入口和状态机

目标：先把工作 / 设计 / 代码三个模式在产品壳里建立起来。

界面方案：

- 使用 iOS 风格 segmented control。
- 展示中文：工作 / 设计 / 代码。
- 内部枚举保留英文：`work | design | code`。
- 默认选中工作。
- 切换时有轻量滑动动画。
- 当前选中项用白色浮层、浅阴影、圆角胶囊。

状态模型：

```ts
type PinvouMode = "work" | "design" | "code";

type CodeAgentProvider =
  | "codex"
  | "claude-code"
  | "cursor"
  | "custom-mcp";

interface PinvouModeState {
  mode: PinvouMode;
  codeProvider?: CodeAgentProvider;
  selectedDesignElementId?: string;
  designRuntimeStatus: "idle" | "injecting" | "ready" | "error";
}
```

模式行为：

- 工作：沿用当前 Pinvou 逻辑。
- 设计：打开预览区设计交互能力，右侧出现设计面板。
- 代码：先展示代码 Agent 选择占位，保留 Codex、Claude Code、Cursor、自定义 MCP。

验收标准：

- 模式切换不影响当前会话内容。
- 切到设计后，输入框 placeholder 变成设计语境。
- 切到代码后，可以选择并保存一个代码 Agent provider。
- 刷新页面后，能恢复最近一次选择的模式或默认回到工作，具体由产品决定。

### 开发阶段 2：设计运行时最小闭环

目标：在设计模式里让用户能点选预览页面元素。

实现内容：

- 在预览页面注入 `design-runtime`。
- runtime 创建独立 overlay root，避免污染产物 DOM。
- hover 时绘制高亮框。
- click 时选中元素，并阻止原页面点击跳转。
- 向 Pinvou shell 发送元素信息。

元素信息结构：

```ts
interface DesignElementSnapshot {
  id: string;
  selector: string;
  tagName: string;
  className: string;
  text: string;
  rect: {
    x: number;
    y: number;
    width: number;
    height: number;
  };
  computedStyle: Record<string, string>;
}
```

验收标准：

- 可以 hover 到真实页面元素。
- 可以点击选中元素。
- 选中后 Pinvou 侧边面板展示元素基础信息。
- 退出设计模式后 overlay、事件监听、临时样式都被清理。

### 开发阶段 3：最小可视化编辑

目标：支持第一批高频设计修改。

第一批控件：

- 文案输入框。
- 文字颜色选择器。
- 背景颜色选择器。
- 字号数字输入。
- 字重选择。
- margin / padding 四向输入。
- width / height 输入。
- border radius 输入。

变更记录：

```ts
interface DesignChange {
  id: string;
  elementId: string;
  selector: string;
  type: "style" | "text" | "comment";
  property?: string;
  oldValue?: string;
  newValue?: string;
  comment?: string;
  status: "todo" | "applied" | "resolved" | "reverted";
  createdAt: string;
}
```

验收标准：

- 修改控件后预览页面即时变化。
- 每条修改都进入 changes log。
- 支持撤销单条修改。
- 支持清空当前页面设计修改。

### 开发阶段 4：应用到代码的占位闭环

目标：先不强依赖完整代码模式，也能把设计修改回流到现有 Pinvou。

实现策略：

- 如果已选择代码 Agent，则生成代码模式任务。
- 如果未选择代码 Agent，则生成工作模式可读任务，回流给当前 Pinvou 修改链路。
- 变更 payload 统一，避免以后重写。

任务文案示例：

```text
请根据以下设计模式变更修改源码。优先修改组件或样式文件，不要只在运行时覆盖样式。

目标元素：.hero-title
页面路径：/
变更：
- font-size: 40px -> 48px
- color: rgb(17, 24, 39) -> #111827
备注：标题需要更有主视觉冲击力。
```

验收标准：

- 点击「应用到代码」能生成稳定任务。
- 任务能进入当前 Pinvou 修改链路。
- 修改完成后刷新预览。
- 已处理变更标记为 resolved。

## 自动化测试方案

### 测试目标

自动化测试要覆盖三件事：

- 模式入口稳定：工作 / 设计 / 代码切换不会破坏现有 Pinvou。
- 设计运行时稳定：overlay 能注入、选中、编辑、清理。
- 应用链路稳定：changes log 能正确生成，并能交给后续修改链路。

### 单元测试

覆盖对象：

- mode reducer / store。
- segmented control 选中状态。
- code provider 保存逻辑。
- selector 生成函数。
- changes log reducer。
- style patch 生成函数。
- design payload serializer。

关键用例：

- 默认模式是工作。
- 切换到设计会设置 `designRuntimeStatus`。
- 切换离开设计会清空 `selectedDesignElementId`。
- 选择 Codex / Claude Code / Cursor / 自定义 MCP 后能持久化。
- 同一元素多次修改能合并或按顺序记录，规则要固定。
- 撤销单条变更后状态变成 `reverted`。

### 组件测试

覆盖对象：

- iOS 风格三段式模式入口。
- 设计面板。
- 代码 Agent 选择面板。
- changes log 面板。

关键用例：

- 点击「工作 / 设计 / 代码」能正确切换 active UI。
- 设计模式下显示设计面板。
- 代码模式下显示 provider picker。
- 设计面板没有选中元素时展示空状态。
- 选中元素后，控件显示当前 computed style。
- 修改控件会触发 design change action。

### 集成测试

覆盖对象：

- Pinvou shell 与 preview iframe/WebView 通信。
- design-runtime 注入和销毁。
- runtime 到 shell 的 postMessage。
- shell 到 runtime 的 style/text patch。

关键用例：

- 进入设计模式后 runtime 注入成功。
- hover 元素后 overlay 坐标正确。
- click 元素后 shell 收到 `DesignElementSnapshot`。
- 修改文字后预览 DOM 文案变化。
- 修改颜色后 computed style 变化。
- 退出设计模式后 hover/click 不再触发设计行为。
- 预览页面刷新后，设计模式能重新注入 runtime。

### E2E 测试

推荐使用 Playwright。

基础流程：

```text
打开 Pinvou
-> 默认处于工作模式
-> 切换到设计模式
-> 等待预览区 runtime ready
-> hover 一个标题
-> click 选中标题
-> 修改字号和颜色
-> 确认页面即时变化
-> 打开 changes log
-> 点击应用到代码
-> 确认生成代码任务或进入占位面板
-> 切回工作模式
-> 确认 overlay 被清理
```

代码模式占位流程：

```text
打开 Pinvou
-> 点击代码
-> 展示 Codex / Claude Code / Cursor / 自定义 MCP
-> 选择 Codex
-> 切回设计
-> 点击应用到代码
-> payload 中 provider 为 codex
```

### 视觉回归测试

目标：防止 overlay、segmented control、设计面板出现布局错乱。

截图点：

- 工作模式默认界面。
- 设计模式未选中元素。
- 设计模式选中元素。
- 代码模式 provider picker。
- 移动宽度下的模式入口。
- 小高度窗口下的设计面板。

检查重点：

- 三段式入口文字不溢出。
- 当前选中浮层位置正确。
- overlay 不遮挡 Pinvou 自身输入框。
- 设计面板不会把预览区挤到不可用。
- changes log 长内容可滚动。

### 兼容性测试

需要覆盖的产物类型：

- React + Vite。
- Next.js。
- Vue + Vite。
- 静态 HTML。
- Tailwind 项目。

需要覆盖的页面情况：

- 普通 DOM。
- 深层嵌套 DOM。
- fixed / sticky 元素。
- transform 过的元素。
- scroll container 内元素。
- shadow DOM 页面，第一版可以只检测并提示不完全支持。

### 错误和降级测试

关键场景：

- 预览区还没有产物。
- iframe 跨域导致无法注入。
- 页面 CSP 阻止脚本注入。
- 用户选中 SVG、canvas、video 等不适合第一版编辑的元素。
- 选中元素后页面重新渲染，原 DOM 消失。
- selector 无法唯一命中。

预期行为：

- 给出明确错误状态。
- 不影响工作模式继续使用。
- 不产生半残留 overlay。
- changes log 不记录失败变更，或明确标记为 failed。

### CI 建议

每次提交跑：

- TypeScript 类型检查。
- 单元测试。
- 组件测试。
- 最小 Playwright E2E。

主分支合并前跑：

- 全量 Playwright。
- 多框架 fixture 测试。
- 视觉截图对比。
- 打包产物检查。

### 测试目录建议

```text
tests/
  unit/
    mode-store.test.ts
    design-changes.test.ts
    selector.test.ts
  component/
    mode-segmented-control.test.tsx
    design-panel.test.tsx
    code-provider-picker.test.tsx
  integration/
    design-runtime-injection.test.ts
    preview-bridge.test.ts
  e2e/
    design-mode-basic.spec.ts
    code-provider-placeholder.spec.ts
    design-apply-to-code.spec.ts
  fixtures/
    react-vite/
    next/
    vue-vite/
    static-html/
```

### 第一批必须自动化的验收用例

- 默认显示工作模式。
- 用户能切换到设计模式。
- 设计模式能注入 runtime。
- 用户能选中预览元素。
- 用户能修改文字颜色并即时看到变化。
- changes log 记录 selector、property、oldValue、newValue。
- 用户能撤销修改。
- 用户能切换到代码模式并选择 Codex。
- 应用到代码时 payload 包含 selected provider。
- 离开设计模式后 overlay 被清理。

## 第二步专项方案：设计 runtime 最小注入闭环

### 目标

第二步不直接引入 `SandeepBaskaran/design-mode` 的完整代码，而是先做 Pinvou 原生的最小 runtime，验证预览区能不能被设计模式控制。

本步骤只解决四件事：

- 注入：进入设计模式时，把 runtime 注入到产物预览页面。
- 选中：用户 hover/click 预览 DOM 元素时有高亮和选中状态。
- 通信：runtime 把选中元素信息通过 `postMessage` 发回 Pinvou。
- 清理：离开设计模式时，overlay、事件监听和临时状态全部移除。

### 开发范围

新增模块：

- `design-runtime.js`：生成可注入到预览页面的 runtime 脚本。
- `design-runtime-state.js`：处理 runtime 消息、元素 snapshot 归一化、注入状态。

修改模块：

- `ArtifactsPanel`：识别 HTML/Web 预览 iframe，并在设计模式开启时注入 runtime。
- `ChatView`：把当前模式传给 `ArtifactsPanel`，并展示 runtime 状态和最近选中元素摘要。

### runtime 行为

runtime 注入后：

```text
创建 overlay root
监听 mousemove
监听 click
hover 元素 -> 绘制蓝色高亮框
click 元素 -> 阻止页面默认点击 -> 固定选中框 -> postMessage 元素信息
收到 destroy 消息 -> 移除 overlay 和事件监听
```

元素 snapshot：

```ts
interface DesignElementSnapshot {
  id: string;
  selector: string;
  tagName: string;
  className: string;
  text: string;
  rect: {
    x: number;
    y: number;
    width: number;
    height: number;
  };
  computedStyle: {
    color: string;
    backgroundColor: string;
    fontSize: string;
    fontWeight: string;
    margin: string;
    padding: string;
    width: string;
    height: string;
    borderRadius: string;
  };
}
```

### 注入策略

优先路径：

- 如果产物预览是同源 iframe，直接通过 `iframe.contentWindow.eval(runtimeScript)` 或向 iframe 文档追加 script。
- Pinvou shell 和 runtime 使用 `window.postMessage` 通信。

降级路径：

- 如果无法访问 iframe document，设计模式状态标记为 `error`。
- UI 显示“当前预览暂不支持直接设计编辑”。
- 不影响工作模式和代码模式继续使用。

### 第二步验收标准

- 切到「设计」后自动打开已有产物预览。
- 预览 iframe 注入 runtime 成功时显示 `ready`。
- hover 预览页面元素时有高亮框。
- click 预览页面元素后，Pinvou 能收到 `DesignElementSnapshot`。
- 输入框上方或设计状态条能展示最近选中的元素，例如 `h1 .hero-title`。
- 切回「工作」或「代码」后 runtime 被销毁。
- 销毁后继续点击预览页面不会触发设计选中。

### 第二步自动化测试方案

单元测试：

- runtime script 生成结果包含启动、选中、销毁消息类型。
- selector 生成函数对 `id`、class、nth-of-type 有稳定输出。
- snapshot 归一化保留必要 computed style 字段。
- reducer 能处理 `runtime-ready`、`element-selected`、`runtime-error`、`runtime-destroyed`。

集成测试：

- 构造一个测试 iframe，注入 runtime。
- 模拟 mousemove，验证 overlay 节点出现。
- 模拟 click，验证父页面收到 `pinvou:design-element-selected`。
- 发送 destroy 消息，验证 overlay 节点消失。

E2E smoke：

```text
打开 Pinvou
-> mock 一个 HTML 产物
-> 打开产物面板
-> 切换到设计模式
-> 等待 runtime ready
-> 在 iframe 内 hover h1
-> click h1
-> 验证 Pinvou 状态条展示 h1 和 selector
-> 切回工作模式
-> 验证 runtime destroyed
```

暂不测试：

- 修改颜色/字号。
- changes log。
- 应用到代码。
- Figma / MCP。

## 第三步专项方案：最小设计面板与 changes log

### 目标

第三步把设计模式从“能选中元素”推进到“能临时编辑元素，并记录结构化设计变更”。

本步骤仍然不改源码，也不接 Codex / Claude。所有修改只发生在产物预览 iframe 内，由 runtime 临时应用。后续第四步再把 changes log 交给工作模式或代码模式落源码。

### 开发范围

新增/扩展能力：

- 选中元素后显示设计面板。
- 支持修改文案。
- 支持修改文字颜色。
- 支持修改背景颜色。
- 支持修改字号。
- 支持修改圆角。
- runtime 接收 patch 消息并应用到选中元素。
- ChatView 记录 changes log。
- 支持清空当前设计修改，恢复预览并清空 changes log。

### 设计面板 UI

设计面板放在输入框上方、设计状态条下面。

无选中元素时：

```text
请选择预览中的元素
```

有选中元素时：

```text
h1 .hero-title

文案：[ Pinvou Design                     ]
文字颜色：[ color picker ]
背景颜色：[ color picker ]
字号：[ 32 ] px
圆角：[ 0 ] px

设计变更 3
- text: Pinvou Design -> Pinvou 设计模式
- font-size: 32px -> 40px
- color: rgb(0,0,0) -> #007AFF

[清空修改]
```

### runtime patch 协议

Pinvou shell 发给 iframe：

```ts
interface DesignApplyChangeMessage {
  type: "pinvou:design-apply-change";
  payload: {
    selector: string;
    changeId: string;
    changeType: "style" | "text";
    property?: string;
    value: string;
  };
}
```

runtime 回传：

```ts
interface DesignChangeAppliedMessage {
  type: "pinvou:design-change-applied";
  payload: {
    changeId: string;
    selector: string;
    ok: boolean;
    error?: string;
  };
}
```

清空修改：

```ts
interface DesignClearChangesMessage {
  type: "pinvou:design-clear-changes";
}
```

runtime 需要保存原始值：

- 文案原始值。
- 每个 inline style 修改前的原始值。

点击清空后按原始值恢复。

### changes log 数据结构

```ts
interface DesignChange {
  id: string;
  elementId: string;
  selector: string;
  type: "style" | "text";
  property?: string;
  oldValue: string;
  newValue: string;
  status: "todo" | "applied" | "failed" | "reverted";
  createdAt: string;
}
```

### 第三步验收标准

- 点选预览元素后，设计面板显示元素 tag、selector 和当前文案。
- 修改文案后，iframe 内元素文本即时变化。
- 修改颜色后，iframe 内元素颜色即时变化。
- 修改字号后，iframe 内元素字号即时变化。
- 修改圆角后，iframe 内元素圆角即时变化。
- 每次修改都进入 changes log。
- changes log 展示 oldValue 和 newValue。
- 点击清空后，iframe 恢复原始状态，changes log 清空。
- 切回工作 / 代码后，runtime 清理，设计面板隐藏或回到对应模式 UI。

### 第三步自动化测试方案

单元测试：

- `buildDesignRuntimeScript` 包含 apply / clear / applied 消息类型。
- style patch 能把 `fontSize`、`color`、`backgroundColor`、`borderRadius` 应用到元素。
- clear changes 能恢复原始 text 和 style。
- changes log reducer 能新增、标记 applied、清空。

E2E smoke：

```text
打开 Pinvou
-> mock 一个 HTML 产物
-> 打开产物面板
-> 切换到设计模式
-> 点击 iframe 内 h1
-> 设计面板显示 h1 selector
-> 修改文案
-> 验证 iframe 内 h1 文案变化
-> 修改字号
-> 验证 computed font-size 变化
-> 修改文字颜色
-> 验证 computed color 变化
-> 验证 changes log 至少 3 条
-> 点击清空修改
-> 验证 h1 文案和样式恢复
-> 验证 changes log 清空
```

暂不测试：

- 真正源码修改。
- Apply to Code。
- 多元素批量修改。
- token / design system。

## 第一版不要踩的坑

- 不要让设计模式直接写源码。它只负责生成结构化设计变更。
- 不要第一版做复杂设计系统和 Figma 导入。
- 不要只做截图批注，否则不像 TRAE 设计模式。
- 不要把入口藏得太深，设计模式应该是和工作 / 代码平级的主模式。
- 不要依赖 selector 作为唯一定位方式，后续需要补 source map、React component stack、framework adapter。

## 结论

Pinvou 第一版设计模式推荐基于 `SandeepBaskaran/design-mode` 的思路做本地集成，辅以 `instruckt` 的源码定位和 annotation 方案。

第一步是先在 Pinvou 产品壳里建立工作 / 设计 / 代码三模式入口和状态机，然后让设计模式能控制预览区进入可视化选择状态。输入框上方可以加设计入口，但它更应该作为全局模式切换的一部分，而不是孤立按钮。
