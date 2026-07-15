# 产物 Markdown 直接编辑开发方案

## 背景

当前 Pinvou3 的产物面板已经支持 Markdown 产物预览，但只是只读渲染：

- 前端入口：`pinvou3-app/src/features/artifacts/ArtifactsPanel.jsx`
- 当前逻辑：`pv.kind === 'md'` 时调用 `bridge.renderMarkdown(pv.text || '')`，再通过 `dangerouslySetInnerHTML` 输出 HTML
- 后端读取：`pinvou3-app/src-tauri/src/commands.rs` 的 `read_artifact_text`
- bridge 暴露：`pinvou3-app/src/tauri-bridge.js` 的 `readArtifactText`

目标改为：Markdown 产物打开后仍然显示渲染后的文档视图，但这个文档视图本身可以直接编辑；鼠标选中文本后出现“AI 编辑”按钮，用户输入修改要求后，把选中文本和要求预填到 AI 对话输入框。

本方案仅描述开发设计，不继续开发实现。

## 目标交互

1. 用户打开 `.md/.markdown` 产物。
2. 面板显示渲染后的 Markdown 文档，不出现“预览 / 编辑”双模式切换。
3. 用户可直接在渲染后的文档上点击、选中、删除、输入和粘贴。
4. 用户手动编辑后，1 秒防抖自动保存回原 Markdown 文件。
5. 用户选中文本后，在选区附近浮出“AI 编辑”按钮。
6. 点击“AI 编辑”后，按钮变成一个输入框。
7. 用户输入修改要求并确认。
8. 系统把文件路径、选中文本、修改要求组织成 prompt，调用 `bridge.prefillComposer(prompt)`。
9. 用户在聊天输入框中检查后，自己点击发送。

第一版不做直接发送给模型。原因：

- 避免误触后直接发出请求。
- 用户可以发送前补充上下文。
- 当前 `prefillComposer` 链路已存在，风险更低。
- 直接发送还要处理 busy 状态、发送失败、会话切换、Agent 写入后重新读取等更多状态。

## WorkBuddy 参考

WorkBuddy 打包代码中已找到两条相关链路。

### Markdown 保存链路

- `FileTabs-hvVOOhm-.js` 的 `MarkdownEditor`
- `FileTabs-hvVOOhm-.js` 的 `useMarkdownEditor`
- `FileTabs-hvVOOhm-.js` 的 `AUTO_SAVE_DELAY = 1000`
- `FileTabs-hvVOOhm-.js` 的 `handleSave -> onSaveFile(filePath, text)`
- `detail-panel-wrapper-PE22ly4v.js` 的 `wrappedSaveFile -> adapter.writeFile(sessionId, filePath, content)`

可借鉴思想：

- 编辑器内部维护编辑状态。
- 用户编辑触发内容变更。
- 1 秒防抖自动保存。
- 宿主提供 `onSaveFile(path, content)`，编辑器不直接理解文件系统。
- 无 `onSaveFile` 时退化为只读。

### AI 选区编辑链路

- 选区浮层：`FileTabs-hvVOOhm-.js` 的 `SelectionQuotePopup`
- AI 编辑按钮：`SelectionQuotePopup` 内的 `selection-quote-trigger`
- 指令输入框：`FileTabs-hvVOOhm-.js` 的 `SelectionQuoteInputBox`
- Markdown 编辑器挂载点：`MarkdownEditorComponent` 在 `!props.readOnly` 时渲染 `<SelectionQuote />`
- Markdown 文件信息打包：`handleSendSelectionQuoteToChat` / `handleInsertSelectionQuoteToInput`
- 宿主发送/插入：`detail-panel-wrapper-PE22ly4v.js` 的 `wrappedSelectionQuoteSend` / `wrappedSelectionQuoteInsert`
- chat 输入块构造：`connector-YkxMjBPL.js` 的 `buildSelectionQuotePhraseBlock`

WorkBuddy 真实行为：

1. 用户在 Markdown 编辑区划选文本。
2. 选区稳定且不是折叠选区时，在选区末尾附近显示“AI 编辑”按钮。
3. 点击后高亮当前选区，并弹出输入框。
4. 用户输入要求。
5. WorkBuddy 把 `selectedText`、文件路径、文件名、定位范围 `quoteRef.locationData` 打包成 `resource_link` phrase block。
6. Agent 后续仍通过正常文件写入链路修改文件，不是在前端立即替换选区文本。

Pinvou3 不照搬 WorkBuddy 的 SmartDoc/Lexical/block range 体系。第一版只借鉴交互：

- 选中文本后显示按钮。
- 按钮转输入框。
- 确认后预填聊天输入框。
- 不做结构化 phrase block。
- 不做点击聊天标签回定位。

## 范围

第一阶段实现：

- 只支持 `.md` / `.markdown` artifact。
- 渲染后的 Markdown 文档直接可编辑。
- 手动编辑自动保存回原文件。
- 支持表格、标题、段落、列表等常见 Markdown 内容的编辑。
- 支持选中文本后显示“AI 编辑”按钮。
- AI 编辑确认后只预填聊天输入框，不自动发送。
- 保存失败有明确状态提示。
- 切换 artifact、关闭面板、失焦前尽量保存。

不做：

- “预览 / 编辑”双模式切换。
- textarea 源码编辑体验。
- 所见即所得完整 Markdown 编辑器能力。
- AI 返回内容的前端自动局部替换。
- 结构化 selection quote block。
- 多人协作或复杂冲突合并。
- 非 artifact 路径写入。
- 远端/云端 sandbox 写入。

## 技术方案总览

### 前端核心组件

新增组件：

`pinvou3-app/src/features/artifacts/EditableMarkdownPreview.jsx`

职责：

- 接收 artifact、Markdown 源文本、主题、文案对象。
- 调用现有 `bridge.renderMarkdown(markdown)` 得到 HTML。
- 用 `contentEditable` 承载渲染后的 HTML。
- 监听用户输入，把 DOM HTML 转回 Markdown。
- 自动保存 Markdown 文本。
- 管理选区 AI 编辑浮层。

建议 props：

```js
function EditableMarkdownPreview({
  artifact,
  initialText,
  initialInfo,
  isDark,
  t,
  onSaved,
  onReloaded,
})
```

父组件通过 ref 暴露：

```js
{
  flush,
  hasDirty,
  reloadFromDisk,
}
```

`ArtifactsPanel.jsx` 的 `pv.kind === 'md'` 分支直接渲染该组件，不再渲染普通只读 `dangerouslySetInnerHTML`，也不再增加 `mdMode`。

### HTML 转 Markdown

因为编辑的是渲染后的 HTML，而保存目标是 `.md`，需要 HTML -> Markdown 转换。

建议新增依赖：

- `turndown`
- `turndown-plugin-gfm`

用途：

- `turndown`：将 contentEditable DOM 转回 Markdown。
- `turndown-plugin-gfm`：支持 GitHub Flavored Markdown，尤其是表格。

转换策略：

```js
const turndown = new TurndownService({
  headingStyle: 'atx',
  bulletListMarker: '-',
  codeBlockStyle: 'fenced',
})
turndown.use(gfm)
const markdown = turndown.turndown(editableEl.innerHTML)
```

注意：

- HTML -> Markdown 可能规范化表格空格、列表缩进、空行。
- 第一版接受格式轻微变化，但不能丢内容。
- 保存前不要把 AI 编辑浮层、按钮、临时 UI 节点放进 editable 根节点内部。

## 后端方案

### 新增 Tauri 命令

在 `pinvou3-app/src-tauri/src/commands.rs` 新增：

```rust
#[tauri::command]
pub async fn write_artifact_text(path: String, content: String) -> Result<(), String> {
    write_artifact_text_impl(&path, &content)
}
```

内部实现要求：

1. 调用 `validate_user_path(&path)`。
2. 校验目标存在且是文件。
3. 校验扩展名只允许：
   - `md`
   - `markdown`
4. 限制写入大小，例如 10 MB。
5. 使用 UTF-8 写回。
6. 使用同目录临时文件写入，再 rename 到目标文件。
7. Windows 上不能直接依赖 rename 覆盖目标，需用备份文件兜底：
   - 写 tmp
   - rename 原文件为 backup
   - rename tmp 为目标
   - 成功后删除 backup
   - 失败时尽量恢复 backup

### 注册命令

在 `pinvou3-app/src-tauri/src/lib.rs` 的 `invoke_handler` 中加入：

```rust
commands::write_artifact_text,
```

### 后端测试

在 `commands.rs` 的 tests 中增加：

- `write_artifact_text_allows_markdown`
- `write_artifact_text_allows_markdown_extension`
- `write_artifact_text_blocks_non_markdown`
- `write_artifact_text_blocks_sensitive_path`
- `write_artifact_text_requires_existing_file`
- `write_artifact_text_cleans_temp_file_on_error`

## Bridge 方案

在 `pinvou3-app/src/tauri-bridge.js` 增加：

```js
function writeArtifactText(path, content) {
  return invoke("write_artifact_text", { path, content });
}
```

并在 `window.TauriBridge` export 中暴露：

```js
writeArtifactText,
```

调用方：

```js
await bridge.writeArtifactText(path, markdown);
```

## 直接编辑保存方案

### 编辑初始化

打开 Markdown artifact：

1. `ArtifactsPanel.preview(a)`
2. `bridge.artifactInfo(a.path)`
3. `bridge.readArtifactText(a.path)`
4. `setPv({ kind: 'md', text, info })`
5. `EditableMarkdownPreview` 用 `bridge.renderMarkdown(pv.text)` 初始化 editable HTML

### 用户编辑

1. 用户在 contentEditable 文档内输入、删除、粘贴。
2. `input` 事件触发。
3. 从 editable DOM 转 Markdown。
4. 更新本地 draft。
5. 设置状态为 dirty。
6. 1 秒防抖调用 `bridge.writeArtifactText(path, draft)`。
7. 保存成功后更新 `pv.text`、`pv.info`、列表 `infos[path]`。

### 保存边界

必须支持：

- 1 秒防抖自动保存。
- blur 时立即保存。
- 切换 artifact 前 flush。
- 切换回列表前 flush。
- 关闭产物面板前 flush。
- 保存中串行化，避免并发写入乱序。

保存状态：

- `已保存`
- `未保存`
- `正在保存...`
- `保存失败`

保存失败：

- 不吞错误。
- 状态显示失败原因。
- 切换/关闭时如果 flush 失败，应阻止无感切换。

## 选区 AI 编辑方案

### 选区监听

在 `EditableMarkdownPreview` 中监听：

- `selectionchange`
- editable 根节点的 `mouseup`
- editable 根节点的 `keyup`

当满足以下条件时显示按钮：

- 当前 selection 不为空。
- selection 不是 collapsed。
- selection 的 anchor/focus 都在 editable 根节点内。
- 选中文本 trim 后非空。
- 当前没有正在输入的 AI 编辑框。

按钮位置：

- 使用 `selection.getRangeAt(0).getBoundingClientRect()`。
- 取选区最后一个 rect 或整体 rect。
- 浮层使用 absolute/fixed 定位在选区右下方。
- 避免超出产物面板边界。

### AI 编辑按钮

默认状态：

- 白色/浅色浮层胶囊。
- 文案：`AI 编辑`。
- 图标可用现有 `Sparkles` 或类似图标。

点击后：

- 不清除选区。
- 记录 `selectedText`。
- 按钮变为输入框。
- 输入框 placeholder：`说说你想怎么修改`。
- 输入框右侧有确认按钮。
- `Enter` 确认。
- `Escape` 取消。

### 确认后的行为

用户确认后：

1. 先调用 `flush()` 保存当前文档，确保 Agent 看到最新文件。
2. 构造 prompt。
3. 调用 `bridge.prefillComposer(prompt)`。
4. 跳转到聊天输入框由现有 `composerPrefill` 机制完成。
5. 不自动发送。
6. 清理浮层和选区状态。

prompt 建议：

````text
请根据要求修改下面这段 Markdown 产物内容。

文件路径：{path}

选中文本：
```markdown
{selectedText}
```

修改要求：
{instruction}
````

关于 composer 草稿：

- 现有 `prefillComposer` 会覆盖聊天输入框内容。
- 第一版在调用前弹确认：
  - 如果用户继续，替换输入框内容。
  - 如果用户取消，不修改输入框。
- 后续可增强为追加到输入框或支持草稿栈。

## `ArtifactsPanel.jsx` 修改方案

当前 `pv.kind === 'md'` 分支：

```jsx
<div
  className="msg-md ..."
  dangerouslySetInnerHTML={{ __html: bridge.renderMarkdown(pv.text || '') }}
/>
```

改为：

```jsx
<EditableMarkdownPreview
  ref={mdPreviewRef}
  artifact={sel}
  initialText={pv.text || ''}
  initialInfo={pv.info}
  isDark={isDark}
  t={t}
  onSaved={(text, info) => {
    setPv((prev) => ({ ...prev, text, info: info || prev.info }));
    if (sel?.path && info) {
      setInfos((prev) => ({ ...prev, [sel.path]: info }));
    }
  }}
/>
```

父级新增：

- `mdPreviewRef`
- `flushMarkdownPreview()`

需要在这些入口调用 flush：

- `preview(a)` 切换文件前
- 切换到列表 tab 前
- 关闭产物面板前
- 当前选中的 artifact 不在新 artifacts 列表前

## 文案方案

新增文案：

- `apMdSaved`
- `apMdSaving`
- `apMdDirty`
- `apMdSaveFailed`
- `apMdAiEdit`
- `apMdAiInstructionPlaceholder`
- `apMdAiConfirm`
- `apMdAiCancel`
- `apMdComposerReplaceConfirm`
- `apMdAiPrompt`

不需要：

- `apMdPreview`
- `apMdEdit`
- `apMdEditPlaceholder`

因为不再有预览/编辑双模式，也不再使用源码 textarea。

## 安全边界

必须满足：

- 所有写入路径必须经过 `validate_user_path`。
- 后端只允许 `.md/.markdown`。
- 前端不根据用户输入拼路径。
- 不允许编辑敏感路径、系统路径、凭据路径。
- 不允许创建新文件，只允许写回已存在 artifact。
- 不把浮层 UI 节点写进 Markdown。
- 保存失败必须可见。

## 冲突与外部修改

第一版处理：

- 打开时读取一次。
- 用户编辑期间不主动监听所有外部变更。
- 每次保存后更新 `lastSavedText` 和 `artifactInfo`。
- 切换/关闭前 flush。

AI 编辑的特殊处理：

- AI 编辑确认前先保存当前文档。
- 之后只预填聊天输入框，不直接发给 Agent。
- 用户真的发送后，Agent 可能会从外部修改同一个文件。
- 用户返回该 artifact 时，可以通过“重新打开该 artifact”或后续自动 reload 看到更新。

后续增强：

- 保存前比较 mtime/size，发现外部修改时提示“重新加载 / 覆盖保存 / 取消”。
- Agent 完成后自动刷新当前 artifact。

## 已打开产出物动态刷新开发方案

### 现状判断

当前产出物相关刷新分成两层：

- `tauri-bridge.js` 已监听 `artifact:disk`，能把新产物或删除事件同步到 `state.artifacts`，从而刷新右侧“产物与代码”的列表。
- `ArtifactsPanel.jsx` 在点击某个产物预览时调用 `artifact_info` / `read_artifact_text` 读取一次。
- `EditableMarkdownPreview.jsx` 已暴露 `reloadFromDisk({ force })`，但当前没有接入 `artifact:disk` 或 Agent 完成事件，因此已打开的 Markdown 预览不会跟随磁盘内容变化自动刷新。

所以需要补的是：**文件变更事件 → bridge 状态快照 → 当前打开预览判断同一路径 → dirty 保护 → reloadFromDisk**。

### 目标

1. Agent 或 watcher 修改当前正在预览的 Markdown artifact 后，预览自动刷新为磁盘最新内容。
2. 如果用户正在编辑且有未保存内容，不能静默覆盖用户草稿。
3. 列表刷新和预览刷新解耦：列表可以因新文件/删除变化刷新；预览只在当前 `sel.path` 命中变更路径时 reload。
4. 不引入轮询，不要求用户手动关闭/重开预览。
5. 保持现有自动保存、AI 编辑预填、切换前 flush 逻辑不被破坏。

### 事件来源

第一版只接入已有的 `artifact:disk`：

```js
listen("artifact:disk", function (e) {
  // payload: { path, event, session_id, ... }
});
```

预期事件：

- `created` / `modified`：刷新列表，并对已打开同一路径预览触发 reload。
- `removed`：刷新列表；如果当前预览的就是被删除文件，显示 missing 状态或退回列表。

Agent 完成事件不单独作为第一版主触发源，因为 Agent 写文件最终会走 file watcher。后续如果发现某些写入不触发 watcher，再补 `chat:done` 后基于当前打开 path 做一次 `artifact_info` / mtime 检查。

### Bridge 状态设计

`tauri-bridge.js` 需要在快照中新增一个轻量字段，例如：

```js
state.artifactChange = {
  seq: 0,
  path: "",
  event: "",
  sessionId: "",
  at: 0,
};
```

要求：

- 每次收到有效 `artifact:disk` 事件都递增 `seq`，即使 `state.artifacts` 列表没有新增条目也要 `notify()`。
- `path` 使用事件 payload 的绝对路径；比较前用现有 `normalizedPath()` 统一分隔符和大小写策略。
- `sessionId` 来自 payload，用于避免后台 session 的文件变化刷新当前 session 面板。
- `event === "removed"` 时也要发出变更，让当前预览能处理删除态。

注意：当前 `trackArtifact()` 遇到同 basename 且路径未变化时不会 `notify()`。动态刷新不能依赖 `trackArtifact()` 是否改变列表，必须单独维护 `artifactChange.seq`。

### ArtifactsPanel 接入

`ArtifactsPanel.jsx` 增加一个 effect：

```js
useEffect(() => {
  const change = bs && bs.artifactChange;
  if (!change || !sel || tab !== "preview") return;
  if (!sameArtifactPath(change.path, sel.path)) return;
  if (change.event === "removed") {
    setPv({ missing: true, info: null });
    return;
  }
  if (pv.kind === "md" && mdPreviewRef.current) {
    mdPreviewRef.current.reloadFromDisk({ force: false });
    return;
  }
  preview(sel);
}, [bs?.artifactChange?.seq]);
```

实现细节：

- 需要本地 `sameArtifactPath(a, b)`，至少处理 Windows `\` / `/` 差异和大小写。
- Markdown 走 `EditableMarkdownPreview.reloadFromDisk({ force:false })`。
- HTML/text/image/pdf/docx/xlsx 等非 Markdown 预览可以复用 `preview(sel)` 重新读取/重新转换。
- effect 不应因为 `pv` 对象每次变化而无限循环；依赖应以 `artifactChange.seq` 为主，必要时用 ref 保存当前 `sel/tab/pv.kind`。

### EditableMarkdownPreview 保护策略

已有 `reloadFromDisk({ force = false })` 的第一行逻辑是：

```js
if (!force && latestDraftRef.current !== lastSavedRef.current) return false;
```

这正好作为第一版 dirty 保护：

- 无未保存内容：自动 reload，DOM、draft、lastSaved、info 同步更新。
- 有未保存内容：不 reload，不覆盖用户草稿。

需要补充 UI 状态：

- 当 reload 被 dirty 拦截时，面板应显示一个轻量提示，例如“文件已在外部更新，保存或关闭后可重新加载”。
- 第一版可以先只记录状态，不强制弹 modal。
- 后续增强再提供“重新加载 / 保留我的编辑 / 覆盖保存”三按钮冲突处理。

### 删除事件处理

当 `artifact:disk` 的 `event === "removed"` 且路径命中当前 `sel.path`：

- 调用 `flushMarkdownPreview()` 没有意义，因为文件已经不存在。
- 当前预览设置为 missing 状态。
- 列表中移除对应项。
- 不自动关闭面板，避免用户不知道发生了什么。

### 验收标准

1. 打开一个 `.md` artifact 预览。
2. 自动化模拟或真实 Agent 修改同一路径 Markdown 文件。
3. 预览内容无需关闭/重开即可变为新内容。
4. `artifact_info.modified` 和底部文件信息同步更新。
5. 用户有未保存编辑时，外部变更不会覆盖当前 DOM 内容。
6. dirty 拦截时有可见状态提示或至少可观测状态，不静默丢变更。
7. 外部删除当前预览文件时，预览显示 missing，不显示旧内容。
8. 非当前路径的 artifact 变更不会刷新当前预览。
9. 后台 session 的 artifact 变更不会刷新当前 active session 的预览。
10. 现有 Markdown 编辑、AI 编辑输入框、分栏 UI 回归测试全部继续通过。

### 自动化测试方案

在 `markdown_artifact_edit_smoke.js` 中增加动态刷新场景：

1. mock `read_artifact_text` 使用可变变量 `window.__MD_READ_TEXT__`。
2. 打开 `meeting.md` 预览，断言初始内容存在。
3. 将 `window.__MD_READ_TEXT__` 改为包含“外部更新后的内容”的 Markdown。
4. 触发 mock Tauri 事件：

   ```js
   window.__TAURI_EVENT_HANDLERS__["artifact:disk"].forEach((handler) => {
     handler({ payload: { path: ARTIFACT_PATH, event: "modified", session_id: "s-md" } });
   });
   ```

5. 等待 UI 更新，断言 contentEditable 内出现“外部更新后的内容”。
6. 再执行 dirty 保护场景：
   - 用户先在预览中输入“本地未保存内容”。
   - mock 外部变更并触发 `artifact:disk`。
   - 断言 contentEditable 仍保留“本地未保存内容”，未被外部内容覆盖。
7. 删除事件场景：
   - 触发 `{ event: "removed" }`。
   - 断言面板显示 missing 文案，不显示旧预览内容。

建议新增断言名称：

- `reloads open markdown preview when current artifact changes on disk`
- `does not overwrite dirty markdown draft on external artifact change`
- `shows missing state when current artifact is removed`

### 实施步骤

1. `tauri-bridge.js`：给 state / snapshot 增加 `artifactChange`。
2. `artifact:disk` listener：有效事件统一 bump `artifactChange.seq` 并 `notify()`。
3. `ArtifactsPanel.jsx`：监听 `bs.artifactChange.seq`，按当前 `sel.path` 决定 reload / missing / ignore。
4. `EditableMarkdownPreview.jsx`：保留 `reloadFromDisk({ force:false })` dirty 保护；必要时暴露 reload 被拦截的返回状态。
5. `ArtifactsPanel.jsx`：显示外部更新被 dirty 拦截的轻量提示。
6. `markdown_artifact_edit_smoke.js`：增加动态刷新、dirty 保护、删除事件测试。
7. 运行：

   ```bash
   cd pinvou3-app
   npm run lint:ui
   npm run build:ui
   npm run test
   node tests/markdown_artifact_edit_smoke.js
   node tests/render_markdown_smoke.js
   ```

### 风险

- watcher 可能会收到本机自己 `writeArtifactText` 触发的修改事件；reload 前必须依赖 `dirty` 判断，避免保存中重入。
- `saveNow()` 成功后也会更新本地 preview，不需要立即二次 reload；外部事件到达时内容相同应无视觉抖动。
- Windows 路径大小写和分隔符必须统一，否则同一路径可能匹配失败。
- 非 Markdown artifact 的重新转换可能较慢，第一版可只保证 Markdown 动态刷新，其他类型保留手动重开。

### 已实现结果

本次已按第一版方案落地：

- `tauri-bridge.js` 增加 `artifactChange` 快照字段；`artifact:disk` 有效事件会递增 `seq`，即使产物列表没有变化也会通知前端。
- `ArtifactsPanel.jsx` 监听 `bs.artifactChange.seq`，命中当前预览路径时自动刷新。
- Markdown 预览调用 `EditableMarkdownPreview.reloadFromDisk({ force:false })`，有本地未保存内容时不会覆盖草稿。
- dirty 拦截时展示“文件已在外部更新”提示。
- 当前预览文件被删除时进入 missing 状态，不继续显示旧内容。
- `markdown_artifact_edit_smoke.js` 已增加并通过动态刷新、dirty 保护、删除态三条自动化断言。

## 已发现 UI 回归修复方案：分栏拖动后左侧用户消息被裁切

### 现象

用户在宽屏模式下打开右侧“产物与代码”面板，并拖动中间分割线调整左右区域宽度后，左侧对话区变窄。此时蓝色用户消息气泡里的 AI 编辑 prompt 可能显示不完整，尤其是包含很长 Windows 文件路径时，例如：

```text
C:\Users\123\.pinvou3\sessions\...\workspace\hello-world.md
```

表现为：

- 用户消息气泡没有按左侧对话容器宽度重新收缩。
- 长路径和长文本没有正确断行。
- 气泡内容被右侧裁掉，看起来像蓝色大块 UI 残缺。

### 原因判断

这不是 AI 编辑输入框本身的问题，也不是选区高亮残留。核心原因应在聊天消息布局：

- 左侧聊天主列在分栏变窄后，某些父容器缺少 `min-w-0`，导致子元素按内容最小宽度撑开。
- 用户消息气泡可能使用了固定宽度、过大的 `max-width`，或没有 `max-width: 100%`。
- 气泡文本区域缺少 `overflow-wrap: anywhere` / `word-break: break-word`。
- 长路径、代码块、pre、inline code 等内容没有针对窄容器做断行或横向滚动策略。
- 父层如果带 `overflow-hidden`，会把超出区域直接裁切。

### 修改范围

优先只修改左侧聊天消息布局，不改 Markdown artifact 编辑器和后端写入逻辑。

重点检查：

- `pinvou3-app/src/features/chat/ChatView.jsx`
- 聊天消息气泡组件或 render 分支。
- 用户消息内容容器。
- markdown 文本渲染容器、代码块、路径文本、tool/card 附近的布局类名。

### 具体修复策略

1. 分栏主布局保证可收缩：

   - 左侧聊天列加 `min-w-0`。
   - 聊天滚动容器加 `min-w-0`。
   - 消息行容器加 `min-w-0 max-w-full`。

2. 用户消息气泡按容器收缩：

   - 气泡外层加 `max-w-full`。
   - 如当前有 `max-w-[80%]` / `max-w-[720px]`，保留桌面宽度上限的同时加 `min-w-0`。
   - 不允许气泡宽度超过当前聊天列可用宽度。

3. 文本内容允许长词断行：

   建议给用户消息正文容器加：

   ```css
   overflow-wrap: anywhere;
   word-break: break-word;
   ```

   Tailwind 可用：

   ```text
   min-w-0 max-w-full break-words [overflow-wrap:anywhere]
   ```

4. 代码块和长路径单独处理：

   - 普通段落和路径：允许 `overflow-wrap:anywhere`。
   - `pre` / fenced code：优先 `overflow-x-auto max-w-full`，避免代码格式被强制打散。
   - inline code：允许断行，避免长路径撑爆气泡。

5. 分栏拖动过程中避免 iframe / 面板抢事件，不改变消息布局语义：

   - 已有拖动逻辑只负责调整右侧宽度。
   - 修复不应依赖拖动结束后手动刷新。
   - 宽度变化后 CSS 自然重排即可。

### 验收标准

自动化或手工验收都必须覆盖：

1. 打开右侧“产物与代码”面板。
2. 拖动中间分割线，把左侧聊天区压窄。
3. 让左侧出现一条包含长 Windows 路径的用户消息，例如 AI 编辑 prompt。
4. 确认蓝色用户消息气泡完整显示在左侧容器内。
5. 确认长路径可断行或在代码块中横向滚动，不被裁切。
6. 确认拖动分割线变宽/变窄时，消息气泡实时适配。
7. 确认右侧 Markdown 预览面板不受影响。

### 建议自动化测试

在现有 Puppeteer smoke 中增加一个窄分栏布局用例：

- 打开含 artifact 的会话。
- 打开“产物与代码”面板。
- 通过 DOM 或鼠标拖动把右侧面板宽度调大，让左侧聊天区变窄。
- 注入一条包含长 Windows 路径的用户消息。
- 读取用户消息气泡和聊天容器的 `getBoundingClientRect()`。
- 断言：

```js
bubbleRect.left >= containerRect.left
bubbleRect.right <= containerRect.right
```

同时检查正文文本仍包含完整路径关键片段，不因裁切丢失。

### 已实现修复

本次按以上方案落地：

- `ChatView.jsx` 的分栏宽度增加左侧聊天栏最小宽度约束，右侧 artifact 面板不能继续挤压到聊天栏不可用。
- 左侧聊天滚动容器、消息列表和用户消息气泡补充 `min-w-0` / `max-w-full`。
- 用户消息正文、编辑态 textarea、转交修订气泡增加 `break-words [overflow-wrap:anywhere]`，长 Windows 路径会在气泡内断行。
- `markdown_artifact_edit_smoke.js` 增加 `user bubble remains inside narrowed chat panel` 用例，模拟窄分栏并断言蓝色用户消息气泡没有越出聊天滚动容器。

## 测试计划

### 后端 Rust 测试

运行：

```bash
cd pinvou3-app/src-tauri
cargo test write_artifact_text
```

覆盖：

- 能写 `.md`。
- 能写 `.markdown`。
- 拒绝 `.txt` / `.html` / `.png`。
- 拒绝敏感路径。
- 拒绝不存在文件。
- 写入失败不残留 tmp。

### 前端验证

运行：

```bash
cd pinvou3-app
npm run lint:ui
npm run build:ui
npm run test
```

如新增 HTML -> Markdown 纯函数，可增加轻量单测覆盖：

- 标题转换。
- 段落转换。
- 列表转换。
- 表格转换。
- 加粗/链接转换。

### 手工验收

1. 让 AI 产出一个 `.md` artifact。
2. 打开产物面板。
3. 确认没有“预览 / 编辑”模式切换。
4. 直接点击渲染后的 Markdown 正文并修改文字。
5. 等待 1 秒，状态变为“已保存”。
6. 重新打开该文件，确认内容已写回。
7. 编辑 Markdown 表格中的单元格文字，保存后确认表格内容没有丢。
8. 选中文本后，确认出现“AI 编辑”按钮。
9. 点击按钮后，确认按钮变成输入框。
10. 输入修改要求并确认。
11. 确认跳转聊天输入框，且内容包含文件路径、选中文本、修改要求。
12. 如果聊天框已有草稿，确认先弹替换确认，取消后草稿保留。
13. 切换 artifact 前有未保存内容，确认先保存再切换。
14. 保存失败时，确认不会无提示关闭或切换。

## 实施步骤

1. 撤掉已实现的 textarea 源码编辑方案：
   - 删除或重写 `MarkdownArtifactEditor.jsx`
   - 移除 `ArtifactsPanel.jsx` 中的 `mdMode`
   - 移除“预览 / 编辑”按钮
2. 保留并整理后端 `write_artifact_text` 命令。
3. 保留 bridge `writeArtifactText`。
4. 安装 `turndown` 和 `turndown-plugin-gfm`。
5. 新建 `EditableMarkdownPreview.jsx`。
6. 接入 `ArtifactsPanel.jsx` 的 md 分支。
7. 实现 contentEditable 输入、HTML -> Markdown、自动保存。
8. 实现选区 AI 编辑按钮和输入框。
9. 补充 i18n 文案。
10. 跑 lint/build/test 和手工验收。

## 风险

- `contentEditable` 的光标和 DOM 行为复杂，尤其是表格、列表、复制粘贴。
- HTML -> Markdown 会规范化格式，可能导致 diff 比较大。
- 表格编辑体验能覆盖基础文字修改，但不承诺完整表格结构编辑。
- 如果把浮层放进 editable DOM 内部，会被保存进 Markdown；实现必须把浮层放在外层 overlay。
- `prefillComposer` 会覆盖聊天输入框，需要确认弹窗保护草稿。
- 自动保存和切换保存必须串行化，避免旧内容覆盖新内容。

## 建议结论

按“渲染态直接编辑 + 自动保存 + 选区 AI 编辑预填聊天输入框”实现。这个方向最贴近截图和用户预期。

第一版不追求完整富文本编辑器能力，但必须保证常见 Markdown 文档、表格文字和段落内容可以直接修改并保存。
