# 品悟 多窗口（拖拽撕离 / Tear-off）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让品悟左侧菜单的任意项可以弹出/撕离成独立的操作系统窗口（本计划覆盖 Phase 0 机制验证 + Phase 1 按钮触发的撕离窗口框架，端到端可用、可测试）。

**Architecture:** 撕离窗口 = 同一个 `index.html` 以 `?detached=1&kind=&id=` 启动，只挂载该 kind 对应的已有面板组件（无侧边栏）。Rust 端一个 `open_detached_window` command 用 `WebviewWindowBuilder` 建窗（label 去重 + 聚焦已存在窗口），照搬现有 `open_artifact_window` 的成熟模式。后端 state 进程内共享、窗口间强独立、不做 live 数据同步。

**Tech Stack:** Tauri v2（`tauri 2.11.1`）、Rust、React（浏览器内 Babel，单文件 `index.html`，无打包）、node+puppeteer-core 冒烟测试、`device_query`（仅 Phase 0 spike 用于验证 X11 全局输入）。

## Global Constraints

- Tauri 版本：`tauri = 2.11.1` / `tauri-build = 2.6.1`，**v2 API**（`WebviewWindowBuilder` / `WebviewUrl::App` / `Manager::get_webview_window`）。
- 前端是**单文件** `pinvou3-app/src/index.html`，React 经 `<script type="text/babel">` 浏览器内编译，**无打包步骤**；所有依赖 vendor 离线，不得引 CDN。
- Tauri window label 字符集仅允许 `a-zA-Z0-9-_`（见现有 `open_artifact_window` 注释）。非法字符必须转义/哈希。
- 撕离窗口必须被 capability 覆盖，否则其 webview 无法 invoke——`capabilities/default.json` 的 `windows` 数组要包含 `detached-*` glob。
- 窗口间**不做 live 数据同步**（白浪确认强独立）；真相源在 Rust 后端，每窗各自拉。
- 文案中英日三语：新增任何用户可见文案要在 `t` 字典三处（zh ~L215 / en ~L428 / ja ~L641）都补。
- 飞书/对外文档用中文（与本项目既有约定一致）；本计划内代码注释保持与周围一致风格。
- 提交粒度：每个 Task 末尾 commit 一次，message 用 `feat(multi-window): …` / `test(multi-window): …` / `chore(multi-window): …`。
- 工作目录：worktree `/home/bailang/WorkSpace/Pinvou3-multiwindow`，分支 `feat/multi-window`。所有路径相对 `pinvou3-app/`。

---

## File Structure

**新建：**
- `pinvou3-app/src-tauri/src/bin/tearoff_spike.rs` — Phase 0 一次性 spike bin，验证 device_query 在本机 X11 读全局鼠标坐标+左键状态。验收后保留作回归参考。
- `pinvou3-app/src-tauri/src/detach.rs` — 撕离窗口的 Rust 模块：`detached_label` / `view_title` 纯函数 + `open_detached_window` command。
- `pinvou3-app/tests/detached_boot_smoke.js` — puppeteer 冒烟：以 `?detached=1` 加载 index.html，断言只渲染目标面板、无侧边栏。
- `pinvou3-app/tests/tearoff_buttons_smoke.js` — puppeteer 冒烟：断言侧边栏各项有"⧉ 弹出"入口且点击以正确参数调 `open_detached_window`。

**修改：**
- `pinvou3-app/src-tauri/Cargo.toml` — 加 `device_query`（dev/spike 用，放 `[dev-dependencies]` 不进生产二进制）。
- `pinvou3-app/src-tauri/src/lib.rs` — `mod detach;` + 在 `generate_handler!` 注册 `detach::open_detached_window`。
- `pinvou3-app/src-tauri/capabilities/default.json` — `windows` 加 `"detached-*"`；`permissions` 加 `"core:webview:allow-create-webview-window"`。
- `pinvou3-app/src/index.html` — ① 启动分支（detached vs 完整 App，~L7561）；② `DetachedShell` + `ViewRegistry` 组件；③ 把 `bs/theme/t` 的获取抽成可复用 hook 供两种 shell 共用；④ 侧边栏各项"⧉ 弹出"入口 + `detachedSet` 角标 + 点击聚焦；⑤ 三语文案补 `tearoffTitle` 等。

**Phase 2/3（不在本计划）：** 原生鬼影拖拽手势 + Wayland 降级 + 各 kind 撕离语义打磨 + 回坞角标交互细化，待 Phase 0 spike 确认机制后单独出计划。Phase 1 用每项一个"⧉ 弹出"按钮触发撕离，把窗口管线与拖拽手势解耦，先交付可用版本。

---

## Phase 0 — Spike：验证 X11 全局输入（go/no-go）

### Task 0: device_query 全局鼠标 spike

唯一真正的未知点：本机 X11 下能否稳定读全局鼠标坐标 + 左键按下状态（Phase 2 鬼影跟随+松手判定的地基）。WebviewWindowBuilder 跨屏建窗已有 `open_artifact_window` 先例，低风险，不在 spike 内。

**Files:**
- Create: `pinvou3-app/src-tauri/src/bin/tearoff_spike.rs`
- Modify: `pinvou3-app/src-tauri/Cargo.toml`

**Interfaces:**
- Produces: 结论（device_query 在本机可用/不可用）；若不可用则 Phase 2 主路径改为 §4.4 降级（拖出即在副屏开窗），写回 spec。

- [ ] **Step 1: 加 device_query 依赖**

在 `pinvou3-app/src-tauri/Cargo.toml` 的 `[dev-dependencies]`（没有则新建该段）加：

```toml
[dev-dependencies]
device_query = "2"
```

> 放 dev-dependencies：spike bin 只在开发期手动跑，不进生产 deb。Phase 2 真正用到时再决定是否升为正式 dependency（届时另起计划）。

- [ ] **Step 2: 写 spike bin**

Create `pinvou3-app/src-tauri/src/bin/tearoff_spike.rs`：

```rust
//! 一次性 spike：验证本机 X11 下 device_query 能读全局鼠标坐标 + 左键状态。
//! 跑法：cd pinvou3-app/src-tauri && cargo run --bin tearoff_spike
//! 预期：移动鼠标 / 按住左键时下面打印的坐标和 down=true 实时变化(跨 3 屏都更新)。
//! Ctrl-C 退出。device_query 在 dev-dependencies，故用 `cargo run` 默认 dev profile 可见。

use device_query::{DeviceQuery, DeviceState, MouseState};
use std::{thread, time::Duration};

fn main() {
    let dev = DeviceState::new();
    println!("移动鼠标到不同显示器、按住/松开左键，观察输出。Ctrl-C 退出。");
    let mut last = (i32::MIN, i32::MIN, false);
    loop {
        let m: MouseState = dev.get_mouse();
        let down = *m.button_pressed.get(1).unwrap_or(&false); // index 1 = 左键
        let cur = (m.coords.0, m.coords.1, down);
        if cur != last {
            println!("x={:>5} y={:>5} left_down={}", cur.0, cur.1, down);
            last = cur;
        }
        thread::sleep(Duration::from_millis(16)); // ~60Hz
    }
}
```

- [ ] **Step 3: 跑 spike，人工验收**

Run:
```bash
cd pinvou3-app/src-tauri && cargo run --bin tearoff_spike
```
Expected：
- 鼠标移到**第二/第三台显示器**（x 进入 1920–3839 / ≥3840 区间，本机布局 HDMI-0@0 / USB-C-1@1920 / USB-C-0@3840）时坐标持续更新；
- 按住左键 `left_down=true`，松开 `false`，实时翻转。
- 全程无 panic。

**Go/No-go：** 三屏坐标都更新 + 左键状态可读 → **GO**，Phase 2 用原生鬼影方案。任一不满足 → 记录现象，Phase 2 改降级方案，并在 spec §4.3/§4.4 标注。

- [ ] **Step 4: Commit**

```bash
cd /home/bailang/WorkSpace/Pinvou3-multiwindow
git add pinvou3-app/src-tauri/src/bin/tearoff_spike.rs pinvou3-app/src-tauri/Cargo.toml
git commit -m "chore(multi-window): Phase0 spike 验证 X11 全局鼠标(device_query)"
```

---

## Phase 1 — 按钮触发的撕离窗口框架（可交付）

### Task 1: capabilities 放行撕离窗口建窗

**Files:**
- Modify: `pinvou3-app/src-tauri/capabilities/default.json`

**Interfaces:**
- Produces: 主窗口可调用建窗 API；label 形如 `detached-*` 的窗口被 default capability 覆盖（其 webview 能 invoke/listen）。

- [ ] **Step 1: 编辑 default.json**

把 `windows` 与 `permissions` 改成：

```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "default",
  "description": "enables the default permissions",
  "windows": [
    "main",
    "detached-*"
  ],
  "permissions": [
    "core:default",
    "dialog:default",
    "core:window:allow-minimize",
    "core:window:allow-toggle-maximize",
    "core:window:allow-close",
    "core:window:allow-start-dragging",
    "core:webview:allow-create-webview-window"
  ]
}
```

> `"detached-*"` glob 让所有撕离窗口继承同一套默认权限（否则它们的 webview 无法 invoke 任何命令）。`core:webview:allow-create-webview-window` 是运行时建窗所需。Phase 2 的 set-position 等权限届时再加。

- [ ] **Step 2: 校验 JSON 合法 + 后端能编译**

Run:
```bash
node -e "JSON.parse(require('fs').readFileSync('pinvou3-app/src-tauri/capabilities/default.json','utf8')); console.log('JSON OK')"
cd pinvou3-app/src-tauri && cargo check
```
Expected：打印 `JSON OK`；`cargo check` 成功（capabilities 在编译期被 tauri-build 校验，非法权限名会在此报错）。

- [ ] **Step 3: Commit**

```bash
cd /home/bailang/WorkSpace/Pinvou3-multiwindow
git add pinvou3-app/src-tauri/capabilities/default.json
git commit -m "chore(multi-window): capabilities 放行 detached-* 建窗"
```

---

### Task 2: Rust `open_detached_window` command

**Files:**
- Create: `pinvou3-app/src-tauri/src/detach.rs`
- Modify: `pinvou3-app/src-tauri/src/lib.rs`（`mod detach;` + 注册 handler）

**Interfaces:**
- Produces:
  - `detach::detached_label(kind: &str, id: Option<&str>) -> String` — 纯函数，返回 `detached-{kind}-{16hexhash(id_or_empty)}`，只含 `a-z0-9-`。
  - `detach::view_title(kind: &str) -> &'static str` — kind → 窗口标题。
  - `#[tauri::command] async fn open_detached_window(kind: String, id: Option<String>, app: AppHandle) -> Result<(), String>` — 建/聚焦撕离窗口。前端调用名 `open_detached_window`，参数 `{ kind, id }`。

- [ ] **Step 1: 写 detached_label + view_title 的失败测试**

Create `pinvou3-app/src-tauri/src/detach.rs`，先只放纯函数签名占位 + 测试：

```rust
//! 撕离窗口（tear-off）：把某个左侧菜单项弹成独立 WebviewWindow。
//! 模式照搬 commands::open_artifact_window（label 去重 + 聚焦已存在窗口）。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// 撕离窗口 label。Tauri label 仅允许 a-zA-Z0-9-_，故 id 用 16 位 hex 哈希而非原样拼接，
/// 避免 id 里的非法字符 / 冲突。同一 (kind,id) → 同一 label，用于去重 + 聚焦。
pub fn detached_label(kind: &str, id: Option<&str>) -> String {
    let mut h = DefaultHasher::new();
    id.unwrap_or("").hash(&mut h);
    format!("detached-{kind}-{:016x}", h.finish())
}

/// kind → 窗口标题。未知 kind 退化为通用标题。
pub fn view_title(kind: &str) -> &'static str {
    match kind {
        "session" => "对话",
        "persona" => "专家",
        "workflow" => "工作流",
        "monitor" => "系统监控",
        "toolstore" => "工具商店",
        "cardpool" => "专家卡牌池",
        "localenv" => "本地环境",
        _ => "PINVOU",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_is_sanitized_and_stable() {
        let a = detached_label("session", Some("s-../etc/passwd 你好"));
        assert!(a.starts_with("detached-session-"));
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
        // 同输入稳定（去重/聚焦依赖此性质）
        assert_eq!(a, detached_label("session", Some("s-../etc/passwd 你好")));
    }

    #[test]
    fn label_differs_by_id_and_kind() {
        assert_ne!(detached_label("session", Some("a")), detached_label("session", Some("b")));
        assert_ne!(detached_label("session", Some("a")), detached_label("workflow", Some("a")));
        assert_ne!(detached_label("monitor", None), detached_label("toolstore", None));
    }

    #[test]
    fn view_title_known_and_fallback() {
        assert_eq!(view_title("workflow"), "工作流");
        assert_eq!(view_title("???"), "PINVOU");
    }
}
```

- [ ] **Step 2: 跑测试确认通过**

Run:
```bash
cd pinvou3-app/src-tauri && cargo test --lib detach::tests
```
Expected：3 个测试 PASS（纯函数已实现）。

> 说明：这里纯函数实现与测试同写、直接 PASS——TDD 在纯逻辑上以"测试先行定义契约"为主；下一步的建窗 command 含真实 WebviewWindow，无法单测，走 Task 4 端到端人工验收。

- [ ] **Step 3: 加 open_detached_window command**

在 `detach.rs` 顶部 `use` 区加，并在 `#[cfg(test)]` 之前追加 command：

```rust
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

/// 建/聚焦某菜单项的撕离窗口。已存在同 (kind,id) 窗口则只聚焦。
/// 撕离窗口加载同一个 index.html，带 ?detached=1&kind=&id=，前端据此只渲染该面板。
#[tauri::command]
pub async fn open_detached_window(
    kind: String,
    id: Option<String>,
    app: AppHandle,
) -> Result<(), String> {
    let label = detached_label(&kind, id.as_deref());
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.set_focus();
        return Ok(());
    }

    // index.html?detached=1&kind=<kind>&id=<id>。id 做 URL 编码，空 id 省略。
    let mut query = format!("detached=1&kind={}", urlencode(&kind));
    if let Some(ref i) = id {
        query.push_str(&format!("&id={}", urlencode(i)));
    }
    let url = WebviewUrl::App(format!("index.html?{query}").into());

    WebviewWindowBuilder::new(&app, &label, url)
        .title(view_title(&kind))
        .inner_size(900.0, 720.0)
        .resizable(true)
        .decorations(true)
        .build()
        .map_err(|e| format!("build detached window: {e}"))?;
    Ok(())
}

/// 极简 URL 编码：只转义 query 里会出问题的字符，足够 kind/id 用。
fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}
```

并加一个 urlencode 的单测进 `mod tests`：

```rust
    #[test]
    fn urlencode_escapes_unsafe() {
        assert_eq!(urlencode("a-b_1.~"), "a-b_1.~");
        assert_eq!(urlencode("a b&c=d"), "a%20b%26c%3Dd");
    }
```

- [ ] **Step 4: 注册模块与 handler**

在 `pinvou3-app/src-tauri/src/lib.rs`：模块声明区（其他 `mod xxx;` 旁）加 `mod detach;`；在 `.invoke_handler(tauri::generate_handler![` 列表里（紧挨 `commands::open_artifact_window,` 之后）加一行：

```rust
            detach::open_detached_window,
```

- [ ] **Step 5: 编译 + 跑全部 detach 测试**

Run:
```bash
cd pinvou3-app/src-tauri && cargo test --lib detach && cargo check
```
Expected：detach 下 4 个测试全 PASS；`cargo check` 成功（handler 注册无误）。

- [ ] **Step 6: Commit**

```bash
cd /home/bailang/WorkSpace/Pinvou3-multiwindow
git add pinvou3-app/src-tauri/src/detach.rs pinvou3-app/src-tauri/src/lib.rs
git commit -m "feat(multi-window): open_detached_window command(label 去重+聚焦)"
```

---

### Task 3: 前端 detached 启动分支 + DetachedShell

**Files:**
- Modify: `pinvou3-app/src/index.html`（启动分支 ~L7561；新增 DetachedShell + ViewRegistry；抽 bs/theme/t 共用 hook）
- Create: `pinvou3-app/tests/detached_boot_smoke.js`

**Interfaces:**
- Consumes: URL query `?detached=1&kind=&id=`（Task 2 产生的窗口 URL）。
- Produces: detached 窗口只渲染目标面板、不渲染侧边栏；`window.__PINVOU_DETACHED__`（boolean）供测试/调试探测。

- [ ] **Step 1: 写 detached 启动冒烟测试（先失败）**

Create `pinvou3-app/tests/detached_boot_smoke.js`，照 `tests/ui_smoke.js` 的 puppeteer + mock TauriBridge 套路：

```javascript
#!/usr/bin/env node
/**
 * detached 启动冒烟：以 ?detached=1&kind=workflow 加载 index.html，
 * 断言 ① window.__PINVOU_DETACHED__===true ② 不渲染侧边栏(无 newChat 按钮) ③ 渲染了工作流面板。
 * 用法：node pinvou3-app/tests/detached_boot_smoke.js
 */
const fs = require('fs'), path = require('path'), os = require('os');
function loadPuppeteer() {
  try { return require('puppeteer-core'); } catch (e) {}
  const npx = path.join(os.homedir(), '.npm', '_npx');
  if (fs.existsSync(npx)) for (const d of fs.readdirSync(npx)) {
    const p = path.join(npx, d, 'node_modules', 'puppeteer-core');
    if (fs.existsSync(p)) { try { return require(p); } catch (e) {} }
  }
  console.error('SKIP: 找不到 puppeteer-core'); process.exit(2);
}
const puppeteer = loadPuppeteer();
const INDEX = 'file://' + path.join(__dirname, '..', 'src', 'index.html') + '?detached=1&kind=workflow';
const CHROME = process.env.CHROME ||
  ['/snap/bin/chromium','/usr/bin/chromium','/usr/bin/chromium-browser','/usr/bin/google-chrome','/usr/bin/google-chrome-stable'].find(p => fs.existsSync(p));
if (!CHROME) { console.error('SKIP: 未找到 chromium'); process.exit(2); }
const PROFILE = fs.mkdtempSync(path.join(os.tmpdir(), 'pinvou-detached-'));

// 最小 mock TauriBridge：available=true，工作流看板给一个空 state，避免组件挂载报错。
function injectSource() {
  return `(function(){
    window.__TAURI__ = { core: { invoke: async()=>({}) }, event: { listen: async()=>(()=>{}) } };
  })();`;
}

(async () => {
  const browser = await puppeteer.launch({ executablePath: CHROME, headless: 'new',
    userDataDir: PROFILE, args: ['--no-sandbox'] });
  const page = await browser.newPage();
  await page.evaluateOnNewDocument(injectSource());
  await page.goto(INDEX, { waitUntil: 'networkidle0' });
  await new Promise(r => setTimeout(r, 1200)); // 等 babel 编译 + 首渲染

  const detachedFlag = await page.evaluate(() => window.__PINVOU_DETACHED__ === true);
  const html = await page.content();
  const hasSidebarNewChat = html.includes('id="root"') && /新对话|New chat/.test(html) && /currentChat|搜索对话/.test(html);

  let ok = true;
  if (!detachedFlag) { console.error('FAIL: __PINVOU_DETACHED__ 未置 true'); ok = false; }
  if (hasSidebarNewChat) { console.error('FAIL: detached 模式仍渲染了侧边栏'); ok = false; }
  await browser.close(); fs.rmSync(PROFILE, { recursive: true, force: true });
  if (ok) { console.log('PASS: detached 启动只渲染面板、无侧边栏'); process.exit(0); }
  process.exit(1);
})();
```

Run（先失败）：
```bash
node pinvou3-app/tests/detached_boot_smoke.js
```
Expected：FAIL（`__PINVOU_DETACHED__` 未定义、侧边栏仍在）。

- [ ] **Step 2: 抽 bs/theme/t 为共用 hook**

`index.html` 里 `App`（~L911）目前在自身内部建立 `bs`（TauriBridge 订阅）、`activeTheme`、`t`（i18n 字典）。把这三者的获取抽成一个顶层 hook，供 `App` 与 `DetachedShell` 共用（DRY）。在 `App` 定义之前插入：

```javascript
    // 撕离窗口与主窗口共用的基础状态：TauriBridge 订阅 + 主题 + i18n。
    // 注:把原 App 内的 bs/activeTheme/t 初始化逻辑搬到这里，App 改为 const { bs, activeTheme, t } = usePinvouBase();
    const usePinvouBase = () => {
      const [bs, setBs] = useState(() => (window.TauriBridge && window.TauriBridge.state) || {});
      useEffect(() => {
        if (!window.TauriBridge || !window.TauriBridge.subscribe) return;
        return window.TauriBridge.subscribe(setBs); // 既有 pub/sub
      }, []);
      const activeTheme = /* 沿用 App 里原本的主题推导（prefers-color-scheme / 设置）*/ useTheme(bs);
      const t = /* 沿用 App 里原本的 t = I18N[lang] 选择 */ useI18n(bs);
      return { bs, activeTheme, t };
    };
```

> 实施提示：`useTheme`/`useI18n` 是占位名——把 `App` 里**现有**的主题与 `t` 计算原样移进来即可，不要新发明逻辑。移完 `App` 内改为从 `usePinvouBase()` 解构，确保主窗口行为零变化（跑一遍既有 `node pinvou3-app/tests/ui_smoke.js` 验证不回归）。

- [ ] **Step 3: 加 ViewRegistry + DetachedShell**

在 `App` 定义之后、`ReactDOM.createRoot` 之前插入：

```javascript
    // kind → 撕离窗口该挂载的面板。复用主窗口同款 View 组件（见主渲染区 currentView 分支）。
    const DETACHED_VIEWS = {
      session:   ({ theme, t, bs, id }) => <ChatView theme={theme} t={t} bs={bs} prefill="" onPrefillConsumed={()=>{}} onOpenEditor={()=>{}} onGotoSettings={()=>{}} onGotoTools={()=>{}} />,
      persona:   ({ theme, t, bs, id }) => <ChatView theme={theme} t={t} bs={bs} prefill="" onPrefillConsumed={()=>{}} onOpenEditor={()=>{}} onGotoSettings={()=>{}} onGotoTools={()=>{}} />,
      workflow:  ({ theme, t, bs, id }) => <WorkflowView theme={theme} t={t} bs={bs} />,
      monitor:   ({ theme, t, bs }) => <MonitorView theme={theme} t={t} bs={bs} />,
      toolstore: ({ theme, t, bs }) => <ToolStoreView theme={theme} onNewChat={()=>{}} />,
      cardpool:  ({ theme, t, bs }) => <CardPoolView theme={theme} t={t} bs={bs} onEquipped={()=>{}} onAICreate={()=>{}} initialMyOnly={false} />,
      localenv:  ({ theme, t, bs }) => <LocalEnvView theme={theme} t={t} bs={bs} />,
    };

    const DetachedShell = ({ kind, id }) => {
      const { bs, activeTheme, t } = usePinvouBase();
      // session/persona：boot 时把该 session 切为 active，让 ChatView 显示它。
      useEffect(() => {
        if ((kind === 'session') && id && window.TauriBridge && window.TauriBridge.switchActiveTo) {
          window.TauriBridge.switchActiveTo(id, { detached: true });
        }
      }, [kind, id]);
      const View = DETACHED_VIEWS[kind] || DETACHED_VIEWS.monitor;
      return (
        <div className="h-screen w-screen flex flex-col" data-theme={activeTheme}>
          <div data-tauri-drag-region className="h-9 shrink-0 flex items-center px-3 text-sm select-none"
               style={{ borderBottom: '1px solid rgba(128,128,128,.2)' }}>
            <span data-tauri-drag-region>{t.tearoffTitle || '撕离窗口'} · {kind}</span>
          </div>
          <div className="flex-1 min-h-0 overflow-auto">
            <View theme={activeTheme} t={t} bs={bs} id={id} />
          </div>
        </div>
      );
    };
```

> `LocalEnvView` 若实际组件名不同，按主渲染区真实组件名替换（实施时 grep `currentView === 'localenv'` 或本地环境对应渲染处确认）。`switchActiveTo` 是 `tauri-bridge.js` 既有函数（见 L300）。

- [ ] **Step 4: 启动分支**

把底部（~L7561）：

```javascript
    const root = ReactDOM.createRoot(document.getElementById('root'));
    root.render(<App />);
```

改为：

```javascript
    const root = ReactDOM.createRoot(document.getElementById('root'));
    const __q = new URLSearchParams(window.location.search);
    if (__q.get('detached') === '1') {
      window.__PINVOU_DETACHED__ = true;
      root.render(<DetachedShell kind={__q.get('kind') || 'monitor'} id={__q.get('id') || ''} />);
    } else {
      root.render(<App />);
    }
```

- [ ] **Step 5: 补三语文案 tearoffTitle**

在 `t` 字典 zh(~L215)/en(~L428)/ja(~L641) 各加一项：
- zh：`tearoffTitle: '撕离窗口',`
- en：`tearoffTitle: 'Detached',`
- ja：`tearoffTitle: '切り離し',`

- [ ] **Step 6: 跑 detached 冒烟 + 主窗口不回归**

Run:
```bash
node pinvou3-app/tests/detached_boot_smoke.js
node pinvou3-app/tests/ui_smoke.js
```
Expected：detached 冒烟 PASS（或缺 chromium 时 exit 2 SKIP——则改用 Step 7 手动验收）；`ui_smoke.js` 仍 PASS（抽 hook 未破坏主窗口）。

- [ ] **Step 7: 手动验收（run-dev）**

Run:
```bash
cd pinvou3-app && ./run-dev.sh
```
在 devtools console 执行 `window.__TAURI__.core.invoke('open_detached_window', { kind: 'monitor' })`，确认弹出一个**只有系统监控面板、无侧边栏**的新窗口。

- [ ] **Step 8: Commit**

```bash
cd /home/bailang/WorkSpace/Pinvou3-multiwindow
git add pinvou3-app/src/index.html pinvou3-app/tests/detached_boot_smoke.js
git commit -m "feat(multi-window): detached 启动分支 + DetachedShell/ViewRegistry"
```

---

### Task 4: 侧边栏"⧉ 弹出"入口 + 角标 + 聚焦/回坞

**Files:**
- Modify: `pinvou3-app/src/index.html`（侧边栏 NavItem ~L1229-1264 + 会话行 ~L1306；App 内加 detachedSet 状态与事件监听）
- Create: `pinvou3-app/tests/tearoff_buttons_smoke.js`

**Interfaces:**
- Consumes: `open_detached_window({ kind, id })`（Task 2）。
- Produces: 每个可撕离项有"⧉"按钮 → 调命令；已撕离项显示角标且点击聚焦（再次调同命令即聚焦，因后端去重）；撕离窗口关闭 → 主窗口去角标。

- [ ] **Step 1: 写按钮冒烟测试（先失败）**

Create `pinvou3-app/tests/tearoff_buttons_smoke.js`：

```javascript
#!/usr/bin/env node
/**
 * 撕离按钮冒烟：主窗口加载，断言侧边栏「系统监控」项旁有 ⧉ 弹出入口(data-tearoff="monitor")，
 * 点击后以 {kind:'monitor'} 调 open_detached_window。
 */
const fs = require('fs'), path = require('path'), os = require('os');
function loadPuppeteer(){ try{return require('puppeteer-core');}catch(e){}
  const npx=path.join(os.homedir(),'.npm','_npx');
  if(fs.existsSync(npx))for(const d of fs.readdirSync(npx)){const p=path.join(npx,d,'node_modules','puppeteer-core');
    if(fs.existsSync(p)){try{return require(p);}catch(e){}}}
  console.error('SKIP: 找不到 puppeteer-core');process.exit(2);}
const puppeteer=loadPuppeteer();
const INDEX='file://'+path.join(__dirname,'..','src','index.html');
const CHROME=process.env.CHROME||['/snap/bin/chromium','/usr/bin/chromium','/usr/bin/chromium-browser','/usr/bin/google-chrome','/usr/bin/google-chrome-stable'].find(p=>fs.existsSync(p));
if(!CHROME){console.error('SKIP: 未找到 chromium');process.exit(2);}
const PROFILE=fs.mkdtempSync(path.join(os.tmpdir(),'pinvou-tearoff-'));
function injectSource(){return `(function(){
  window.__CALLS__=[];
  window.__TAURI__={core:{invoke:async(cmd,args)=>{window.__CALLS__.push({cmd,args});return {};}},event:{listen:async()=>(()=>{})}};
})();`;}
(async()=>{
  const browser=await puppeteer.launch({executablePath:CHROME,headless:'new',userDataDir:PROFILE,args:['--no-sandbox']});
  const page=await browser.newPage();
  await page.evaluateOnNewDocument(injectSource());
  await page.goto(INDEX,{waitUntil:'networkidle0'});
  await new Promise(r=>setTimeout(r,1500));
  const btn=await page.$('[data-tearoff="monitor"]');
  let ok=true;
  if(!btn){console.error('FAIL: 没找到 [data-tearoff="monitor"] 入口');ok=false;}
  else{
    await btn.click(); await new Promise(r=>setTimeout(r,200));
    const calls=await page.evaluate(()=>window.__CALLS__.filter(c=>c.cmd==='open_detached_window'));
    if(!calls.some(c=>c.args&&c.args.kind==='monitor')){console.error('FAIL: 点击未以 kind=monitor 调 open_detached_window，实际:',JSON.stringify(calls));ok=false;}
  }
  await browser.close();fs.rmSync(PROFILE,{recursive:true,force:true});
  if(ok){console.log('PASS: ⧉ 弹出入口调用正确');process.exit(0);} process.exit(1);
})();
```

Run（先失败）：
```bash
node pinvou3-app/tests/tearoff_buttons_smoke.js
```
Expected：FAIL（无 `[data-tearoff="monitor"]`）。

- [ ] **Step 2: 加撕离辅助函数 + detachedSet 状态**

在 `App`（~L911）内、return 之前加：

```javascript
      const [detachedSet, setDetachedSet] = useState(() => new Set());
      const tearOff = (kind, id) => {
        const inv = window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke;
        if (!inv) return;
        inv('open_detached_window', id != null ? { kind, id } : { kind });
        setDetachedSet(prev => new Set(prev).add(kind + ':' + (id ?? '')));
      };
      // 撕离窗口关闭 → 回坞去角标（撕离窗口在 unload 时 emit 'detach:closed'，见下）。
      useEffect(() => {
        if (!window.__TAURI__ || !window.__TAURI__.event) return;
        let un;
        window.__TAURI__.event.listen('detach:closed', (e) => {
          const key = e && e.payload; if (!key) return;
          setDetachedSet(prev => { const n = new Set(prev); n.delete(key); return n; });
        }).then(f => un = f);
        return () => { if (un) un(); };
      }, []);
```

- [ ] **Step 3: 给 NavItem 加 ⧉ 入口**

NavItem 组件（侧边栏各项用它，~L1240-1264 调用处）加一个可选的撕离按钮。在 NavItem 定义里，标签右侧渲染（仅当传了 `onTearOff`）：

```javascript
        {onTearOff && (
          <button
            data-tearoff={tearoffKind}
            title={t.tearoffHint || '弹出为独立窗口'}
            onClick={(e) => { e.stopPropagation(); onTearOff(); }}
            className="ml-auto opacity-0 group-hover:opacity-60 hover:opacity-100 transition-opacity px-1"
          >⧉{detached ? ' •' : ''}</button>
        )}
```

并在各 NavItem 调用处补 `onTearOff`/`tearoffKind`/`detached`，例如系统监控（~L1240）：

```javascript
              <NavItem
                active={currentView === 'monitor'}
                onClick={() => setCurrentView('monitor')}
                onTearOff={() => tearOff('monitor')}
                tearoffKind="monitor"
                detached={detachedSet.has('monitor:')}
                /* …原有 icon/label 等 props 保持… */ />
```

对 `cardpool`/`workflow`/`toolStore` 同法补（`tearoffKind` 用后端约定的小写：`cardpool`/`workflow`/`toolstore`）。

> 注意：NavItem 根节点要带 `group` class，`group-hover` 才生效。若现有 NavItem 根节点没有 `group`，加上。

- [ ] **Step 4: 会话行加撕离入口**

会话列表项（~L1306 一带，每个 `chat`）末尾加：

```javascript
                  <button
                    data-tearoff="session"
                    title={t.tearoffHint || '弹出为独立窗口'}
                    onClick={(e) => { e.stopPropagation(); tearOff('session', chat.id); }}
                    className="opacity-0 group-hover:opacity-60 hover:opacity-100 px-1"
                  >⧉{detachedSet.has('session:' + chat.id) ? ' •' : ''}</button>
```

- [ ] **Step 5: 撕离窗口关闭时通知主窗口回坞**

在 `DetachedShell`（Task 3）内加 unload 时 emit。在 `DetachedShell` 的 `useEffect` 里追加：

```javascript
      useEffect(() => {
        const key = kind + ':' + (id || '');
        const onUnload = () => {
          try { window.__TAURI__ && window.__TAURI__.event && window.__TAURI__.event.emit('detach:closed', key); } catch (_) {}
        };
        window.addEventListener('beforeunload', onUnload);
        return () => window.removeEventListener('beforeunload', onUnload);
      }, [kind, id]);
```

> emit 需要 `core:event:allow-emit`（`core:default` 通常已含；若 Task 1 后 emit 报权限，则在 default.json permissions 补 `core:event:allow-emit`）。

- [ ] **Step 6: 补撕离提示文案**

`t` 字典 zh/en/ja 各加 `tearoffHint`：zh `'弹出为独立窗口'` / en `'Pop out to its own window'` / ja `'別ウィンドウに切り離す'`。

- [ ] **Step 7: 跑按钮冒烟 + 主窗口不回归**

Run:
```bash
node pinvou3-app/tests/tearoff_buttons_smoke.js
node pinvou3-app/tests/ui_smoke.js
```
Expected：按钮冒烟 PASS（或缺 chromium SKIP→走 Step 8 手动）；`ui_smoke.js` PASS。

- [ ] **Step 8: 手动端到端验收（run-dev）**

Run `cd pinvou3-app && ./run-dev.sh`，逐项验：
1. 鼠标悬停"系统监控"出现 ⧉，点击 → 弹出独立监控窗口；该项出现 `•` 角标。
2. 再点同一 ⧉ → **不新建**，聚焦已有窗口（label 去重生效）。
3. 关闭撕离窗口 → 主窗口该项 `•` 角标消失。
4. 悬停某会话行 ⧉ → 弹出该对话独立窗口，可正常聊天（与主窗口并存、互不串台）。
5. 把撕离窗口手动拖到第二台显示器并排——确认能正常使用（Phase 1 暂靠系统标题栏拖动）。

- [ ] **Step 9: Commit**

```bash
cd /home/bailang/WorkSpace/Pinvou3-multiwindow
git add pinvou3-app/src/index.html pinvou3-app/tests/tearoff_buttons_smoke.js
git commit -m "feat(multi-window): 侧边栏 ⧉ 弹出入口 + 角标 + 去重聚焦/回坞"
```

---

## Self-Review（已对照 spec 核查）

- **Spec §1/§3 各 kind 撕离**：Task 3 ViewRegistry 覆盖 session/persona/workflow/monitor/toolstore/cardpool/localenv；Task 4 给 monitor/cardpool/workflow/toolstore/session 加入口（persona 等的入口在 Phase 3 卡牌池内细化，已在"Phase 2/3 不在本计划"注明）。
- **Spec §4.2 去重/聚焦/回坞**：Task 2 label 去重+聚焦；Task 4 角标+点击聚焦+关闭回坞。
- **Spec §4.3 原生鬼影拖拽**：本计划只做 Phase 0 spike 验证地基（Task 0），手势本体明确划到 Phase 2 后续计划——避免对 spike 结果做占位式臆测。
- **Spec §5 权限**：Task 1 加 create-webview-window + detached-* glob；set-position 等留 Phase 2（本计划建窗用固定 inner_size，不需要）。
- **Spec §4.5 跨窗协调**：Task 4 用 `detach:closed` event 回坞；不做 live 数据同步（符合"窗口强独立"）。
- **类型一致**：`open_detached_window({kind, id})` 在 Task 2 定义、Task 3/4 调用一致；`detached_label`/`view_title` 命名前后一致；`detachedSet` key 统一为 `kind + ':' + (id||'')`（Task 2 后端 label 用哈希、前端 set 用明文 key，两者用途不同不冲突）。
- **Placeholder 扫描**：`useTheme`/`useI18n`/`LocalEnvView` 三处显式标注为"按现有真实逻辑/组件名替换"，非遗留占位——实施第一步即 grep 替换。
