# 品悟 多窗口（拖拽撕离 / Tear-off）设计

- 日期：2026-06-25
- 分支 / worktree：`feat/multi-window` @ `/home/bailang/WorkSpace/Pinvou3-multiwindow`（基于 main `e6246e2`）
- 状态：设计已与白浪确认，待写实施计划

## 1. 目标与非目标

### 目标
让 **任意左侧菜单项**（会话 session / 专家卡 persona / 工作流 workflow / 系统监控 monitor / 工具商店 / 专家卡牌池 / 本地环境 …）可以**从侧边栏拖拽撕离**成一个独立的操作系统窗口，并能**拖到另一台显示器上松手即在那一屏落位**。撕离后的窗口可与主窗口并排实时使用。

### 非目标
- 不做窗口内分屏 / 多 Tab（那是另一种"多窗口"，本次不做）。
- 不做撕离窗口之间的实时数据双向同步——**窗口之间独立性很强**（白浪确认）。真相源在 Rust 后端、进程内共享；每个窗口各自拉数据即可，无需窗口间 live sync。
- 不改造现有各面板的业务逻辑，只复用。

## 2. 关键事实（第一性，已核实）

- Tauri **v2**（`tauri = 2.11.1`，`tauri-build = 2.6.1`）。前端是单个 `pinvou3-app/src/index.html`，React 经浏览器内 Babel（`type="text/babel"`，无打包步骤），全部 vendor 离线。
- 后端 `src-tauri/src/engine_pool.rs` **已是多 session 并发**：每个 `session_id` 一个独立 keep-alive engine，`commands.rs` 按 `session_id` 路由。→ 撕离会话窗口的后端路由**零改动**。
- 现有 `tauri.conf.json` 只声明一个 `label:"main"` 窗口。`capabilities/default.json` 当前给了 `core:default` + window 的 minimize/toggle-maximize/close/start-dragging，**没有运行时建窗权限**——需补。
- 运行环境（本机）：**X11**，3 台 1920×1080 显示器横向拼接（HDMI-0 @0,0 / USB-C-1 @1920,0 / USB-C-0 @3840,0）。X11 下全局鼠标坐标+按键状态可读 → "原生跟随鬼影"方案可行。

## 3. 核心抽象：菜单项 = 视图描述符（ViewDescriptor）

每个可撕离菜单项统一抽象成 `{ kind, id }`：

| kind | id 含义 | 撕离窗口内容 |
|---|---|---|
| `session` | session_id | 该对话，完整可实时聊（engine_pool 已按 session_id 并发） |
| `persona` | persona id | 已加持该专家的**新对话** |
| `workflow` | run_id 或 workflow id | 独立跑/监控该工作流（进度 / 奏折 / 产物） |
| `monitor` | — | 系统监控面板独立成窗 |
| `toolstore` | — | 工具商店面板 |
| `cardpool` | — | 专家卡牌池面板 |
| `localenv` | — | 本地环境面板 |

设计原则：**新增 kind = 在一张映射表里加一行 + 指定它复用哪个已有面板组件**，不写新业务 UI。

## 4. 架构

### 4.1 撕离窗口 = detached 模式的同一个 index.html
- 撕离窗口 URL：`index.html?detached=1&kind=<kind>&id=<id>`。
- App 启动读 `URLSearchParams`：
  - 有 `detached` → 渲染 `<DetachedShell kind id>`：无侧边栏、无主 chrome，只有
    1. 一条极简标题栏（可 `startDragging` 移动 + 关闭按钮 + 标题）；
    2. 该 kind 对应的那一个面板组件。
  - 无 `detached` → 渲染现有完整 App（含侧边栏）。
- `DetachedShell` 内部维护一张 `kind → 面板组件 + props(id)` 映射表（ViewRegistry），是 §3 表格的代码落地。

### 4.2 窗口身份 / 去重 / 回坞
- 撕离窗口 `label = detached:{kind}:{id}`，全局唯一。
- 撕离同一项时若该 label 已存在 → 不新建，`set_focus` 聚焦已有窗口。
- 主窗口侧边栏：被撕离的项**保留在列表**但显示"已弹出 ⧉"角标；点它 = 聚焦其撕离窗口，而非内联打开。
- 关闭撕离窗口 → emit 回坞信号给主窗口，去角标、恢复内联可点。
- 主窗口维护一个 `detachedSet`（已撕离项集合），来源 = 启动时枚举现存 `detached:*` 窗口 + 监听撕离/关闭事件增量更新。

### 4.3 拖拽机制：原生跟随鬼影（Rust 主导）
难点：webview 里 JS 在鼠标离开窗口后收不到另一屏的松手。解法是把"跟随 + 松手判定"交给 Rust。

抽象 `trait DragTracker`：
- `position() -> (i32, i32)`（全局虚拟桌面物理像素）
- `is_primary_button_down() -> bool`
- X11 实现：`device_query`（硬件状态轮询，**不受 webview 的 X pointer grab 影响**，所以监视器2上的松手可稳定捕获）。
- 降级实现：无全局输入能力（Wayland / 受限环境）时返回不可用 → 走 §4.4 降级路径。

撕离拖拽流程：
1. 侧边栏项 `pointerdown` → 前端记录起点；`pointermove` 超过阈值(~5px) → 调 Rust command `begin_detach_drag(kind, id)`。
2. Rust 起一个**鬼影窗口**：`transparent + decorations:false + always_on_top + skip_taskbar + set_ignore_cursor_events(true)`（click-through，不抢焦点），内容为该项图标+名字的半透明卡片。
3. Rust 后台任务 ~60Hz 循环：`tracker.position()` → 移动鬼影跟随光标跨屏；同时读 `is_primary_button_down()`。
4. 检测到左键**释放**：读最终全局坐标 `(gx, gy)`，
   - 落点在主窗口外接矩形之外 → 创建撕离 `WebviewWindow`（`.position(gx, gy)` 物理像素 = 松手那一屏），销毁鬼影，emit `detach:created {kind,id}` 给主窗口。
   - 落点在主窗口内 → 取消（视为误拖），销毁鬼影，无操作。
5. 异常兜底：鬼影循环要有取消句柄（窗口 close / 按 Esc / 超时）防止僵尸任务。

坐标空间：X11 下 `device_query` 全局坐标与 Tauri `.position()` 的物理虚拟桌面坐标一致，直接用，不做换算。

### 4.4 Wayland / 受限环境降级
`DragTracker` 不可用时，撕离改为：拖出侧边栏即触发，撕离窗口开在**副屏**（`available_monitors` 选非主屏，否则主屏居中），或记忆该项上次位置。功能不丢，只是少了"拖到哪松手就在哪"的手势感。

### 4.5 跨窗口协调（极薄）
窗口间**不做 live 数据同步**。后端 state 进程内共享，每窗各自走 command 拉数据 + `listen` 事件按 `session_id`/`run_id` 过滤。窗口间只传三类极薄信号（Tauri event emit 到指定 label）：
- `detach:created {kind,id}` / `detach:closed {kind,id}`：主窗口更新 `detachedSet` 角标。
- `focus:request {kind,id}`：主窗口点已弹出项 → 聚焦对应撕离窗口。

需核实现有事件是否都带 `session_id`/`run_id`（会话事件应已带；workflow 事件按 run_id）。不带的补 key——这是**唯一**可能动到现有后端的地方，范围极小。

## 5. 权限增补（`capabilities/default.json`）
- `core:webview:allow-create-webview-window`
- `core:window:allow-set-position`、`-set-size`、`-set-always-on-top`、`-set-ignore-cursor-events`、`-set-focus`、`-show`、`-close`、`-destroy`
- 事件 `core:event:allow-emit`、`-listen`（多半已在 `core:default`，缺则补）

## 6. 新增/改动文件清单（预估）

### Rust（`src-tauri/src/`）
- 新增 `tearoff/mod.rs`（或 `detach.rs`）：`begin_detach_drag` / `cancel_detach_drag` command、鬼影窗口管理、`DragTracker` trait + X11(device_query) 实现 + 降级实现、松手落点判定与建窗。
- `commands.rs` / `lib.rs`：注册新 command；建窗辅助（label 规则、去重 focus）。
- `Cargo.toml`：加 `device_query`（X11 全局输入）。
- `capabilities/default.json`：补 §5 权限。

### 前端（`pinvou3-app/src/`）
- `index.html`：启动分支（detached vs 完整 App）；`DetachedShell` + `ViewRegistry`；侧边栏项 pointer 拖拽起手 + 阈值 + 调 `begin_detach_drag`；主窗口 `detachedSet` 角标与点击聚焦逻辑；监听三类协调事件。
  - 注意：index.html 已 7500+ 行，单文件。新代码尽量内聚成可识别的段落/组件，避免散落。
- `tauri-bridge.js`：若需暴露新 command 的 JS 包装。

## 7. 落地分期（降风险）

- **Phase 0 — Spike（机制验证）**：本机 X11 上验证 ① `device_query` 全局坐标 + 左键状态读取；② Tauri 跨屏 `WebviewWindow.position()` 真能落到监视器2；③ 鬼影窗口 transparent + always_on_top + ignore_cursor_events 行为正确。机制证伪就回退 §4.4 为主路径。
- **Phase 1 — 撕离窗口框架**：detached 模式 + `DetachedShell` + `ViewRegistry` + 权限 + label 去重/聚焦/回坞。**先用每项一个"⧉ 弹出"按钮**触发撕离（固定位置开窗），把窗口管线与拖拽**解耦**先跑通。
- **Phase 2 — 原生鬼影拖拽**：在 Phase 1 之上叠加 §4.3 拖拽手势 + §4.4 降级。
- **Phase 3 — 各 kind 覆盖 + 打磨**：补全 persona/workflow/monitor/toolstore/cardpool/localenv 的撕离语义、角标交互、关闭回坞、异常兜底。

## 8. 验证方式（无 GUI 自测难点）
- Rust 逻辑（落点判定、label 规则、去重）走单元测试。
- DragTracker X11 实现走本机手动 spike + 一个可独立运行的小 bin 验证（参考 repo 既有 `src-tauri/src/bin` 习惯）。
- 端到端撕离手势需在白浪的 X11 三屏机器上肉眼验收（Phase 0 与每 Phase 末）。

## 9. 待实现时再定的小问题
- 鬼影卡片的具体视觉（图标来源 / 文案）——实现时按现有侧边栏项样式取。
- workflow 的 id 到底用 run_id 还是 workflow 定义 id（取决于"撕离的是某次运行还是工作流入口"）——Phase 3 看现有 workflow 面板的数据模型定。
