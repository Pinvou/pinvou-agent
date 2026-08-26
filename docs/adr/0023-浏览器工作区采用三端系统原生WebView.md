# 浏览器工作区采用三端系统原生 WebView

Pinvou 的任务浏览器采用 Windows WebView2、macOS WKWebView 与 Linux WebKitGTK 承载真实页面，并由公共 BrowserService 统一任务会话、标签、身份、控制权、安全和核心工具契约；三端核心浏览器能力以 capability 显式声明，不能从“能显示页面”推断“Agent 能自动化页面”。我们不采用外部 Chrome 连续截图作为展示或故障回退，也不在当前架构中引入尚不成熟且显著增加体积与维护成本的 CEF；因此用户无需安装 Chrome。

## 当前实现边界

| Capability / 构建产物 | Windows | macOS | Linux |
|---|---|---|---|
| `browser_native_display` | 开启（WebView2） | 关闭（承载层已编译，产品入口未开放） | 开启（WebKitGTK） |
| `browser_agent_automation` | 开启 | 关闭 | 开启（BrowserCore + WebKitWebDriver） |
| `browser_cdp` | 开启，仅应用自有 WebView2 回环端点 | 关闭 | 关闭 |
| `chrome-devtools-mcp` adapter | 构建期准备并随应用打包 | 不准备、不打包、不启动 | 不准备、不打包、不启动 |
| 外部 Chrome 回退 | 禁止 | 禁止 | 禁止 |

Linux 已接入统一的 Pinvou BrowserCore：DOM/结构读取在任务自有 WebKitGTK 页面内完成，点击、填写、输入和按键经开源 WebKitWebDriver 的元素语义端点产生 `isTrusted=true` 的页面事件；浏览器弹窗通过同一 operation gate 内的 W3C alert 端点接受、拒绝或填写 prompt。任务隔离、多标签、隐藏页面操作与重启恢复复用公共宿主。`.deb` 将 `webkit2gtk-driver` 作为必需依赖，开发环境缺失时不注册浏览器 MCP。当前 WebKitGTK 的 W3C pointer actions 在实机上不能可靠完成，因此 Linux 暂不向 Agent 暴露 `hover` 和 `drag`，不得以 JavaScript 合成事件冒充；`resize_page` 的上游语义是改变顶层窗口的内容 viewport，而 Linux 页面是由右侧 Dock 管理边界的嵌入式子 WebView，因此也从 Linux 目录隐藏并对直呼明确返回 unsupported，不能用改变主窗口或临时覆盖 Dock bounds 冒充。macOS 仍只有可编译承载层，产品入口和 Agent 后端保持关闭。Chrome DevTools 专属高级诊断仍只在 Windows 按 capability 开放。

Linux WebDriver session 崩溃后，宿主为安全重建 WebView 与 WebDriver handle 的映射，会把待绑定页面短暂导航到一次性的随机内部 marker，再重新加载原 URL；该恢复不承诺保留表单、Canvas 或 JavaScript 内存状态。marker 只存在于宿主控制的临时 URL，不向远程页面主世界注入稳定身份或绑定脚本。

## 生命周期与控制边界

- 重启恢复只持久化 URL、标签顺序和 active index，并使用当前用户私有权限原子写入；新进程创建新的 WebView、tab token、targetId 和 lease，旧运行期映射永不复用。恢复后先进入未认领控制态，下一次真实用户操作或 Agent lease 原子认领，重启本身不算用户接管。正常退出保留恢复清单，显式停止或删除任务必须删除；任务删除还要清理会话 MCP 配置。
- Agent 新建标签采用隐藏 staging：先发现应用内 WebView target、由宿主提交请求 URL 首航，再用创建前的完整 lease 做最终 CAS 发布。晚到取消只能在 creation revision 仍未被用户接管或后续 mutation 改变时回滚。
- 已开始的单个工具是原子 dispatch。用户接管使 lease 失效并阻止下一项工具，但不承诺中断已经提交给平台后端的当前调用；`finally` 立即撤销 active operation，只有确实派发过原生输入的调用保留不超过 100ms 的 post-dispatch callback grace。显式 UI 接管立即清除该窗口；750ms 只是 dispatch 异常退出时的保险上限。
- 已 `begin` 的 dispatch 收到外部取消或内部超时后只合作取消上游；wrapper 必须等真实终态，或先终止并确认上游子进程已退出，再执行 `end_agent_operation`。上游成功结果保持权威；失败而提交状态未知时返回不可重试的 commit-unknown 结果，不能用普通错误诱导 Agent 重放。子进程崩溃、stdin 关闭、watchdog 与进程信号共用同一 graceful-shutdown barrier。
- 原生 mutation 与后置观察分属两个提交阶段。mutation 成功后，即使可选快照读取失败也返回已提交成功和 `observationWarning`，禁止重放动作；批量 `fill_form` 先完整校验参数，中途失败返回带 `completedCount`/`failedIndex` 的不可重试部分结果，避免重复填写已经成功的字段。
- popup 不使用短期布尔值授权 Agent mutation：只有处在已 `begin` 且完整 lease 仍有效的原子 dispatch 内，才沿用 Agent 身份走隐藏 staging + 最终 CAS；其余页面自发 popup 转成任务内 User 标签。CAS 前用户已接管则拒绝晚到发布。远程页面不获得全局剪贴板读取权限，内嵌下载默认拒绝并提示改用系统浏览器。
