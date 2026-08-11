# pinvou 工具调用失败发现与解决记录

> agent 工具调用失败的问题 × 方案登记(同一池)。追加规范见文末。

## 池(问题 × 方案)

### 1. `node -e` 内嵌 bash 转义在 cmd 下失效(2026-08-11)

**现象**:统计 TS/TSX LOC 的 `node -e` 命令失败(exit 1),`SyntaxError: Invalid string escape`。

**根因**:
- **执行环境**:底座 exec_shell 在 Windows 用 cmd.exe /C 执行([shell.rs](CodeWhale/crates/tui/src/tools/shell.rs),issue #1691);cmd 不认 bash 的 `\"` 转义(`\` 普通字符、`"` 引号开关)
- **语法层**:`"` 提前结束字符串 → 参数重新分词,node 收到残缺 JS(最小复现 + argv 字节证据确认)
- **模型侧**:训练语料 bash 主导,生成默认 bash 方言;Environment 块有 platform/shell **事实**但缺**行为指令**

**方案**(两个):

- **① 命令规范**(prompt 层,零底座,**✅ 已落地**):`#[cfg(windows)]` 编译期条件注入 instructions(非 Windows 构建编译空函数,零影响),把平台事实升级为行为指令,重开会话生效。落地:runtime_bundle/platform/mod.rs `shell_dialect_hint()` + bridge.rs `build_session_system_prompt` 拼装末尾追加(两模式统一);commit `e5777588`。标题用**【命令规范】**而非"方言"(弱模型对抽象词歧义)。文案:
  ```
  【命令规范】当前 shell 为 cmd.exe(Windows),命令书写遵循 cmd 规则:
  - 命令名用 Windows 版:dir(非 ls)、type(非 cat)、findstr(非 grep)、del(非 rm)、copy(非 cp)、fc(非 diff);
  - 无 bash 转义:\ 是普通字符," 是引号开关(不成对吞参数),参数内引号用 "";转义符是 ^;
  - 不支持 $() 命令替换、单引号包裹、heredoc(<<EOF);变量用 %VAR%(如 %USERPROFILE%);
  - 多行命令用 & 连接或行尾 ^ 续行(cmd 支持多行)。
  ```
- **② bash 优先 + fallback cmd**(动底座,提上游,⏳ 暂不实施):Windows 上 exec_shell 检测 bash.exe 可用则 `bash -c` 执行、否则 fallback cmd。**固定策略**、非跟随用户环境——避免 cmd/bash/powershell 三态漂移(提示词无法适配三态、支持面×3、跨用户不可预测);Environment 显示向执行对齐(显示=执行,消除"显示 bash 执行 cmd"矛盾)。**fallback 只用 cmd 而非 PowerShell 的原因**:
  - **ExecutionPolicy**:PowerShell 有执行策略,企业环境常见 Restricted 直接拦执行;cmd 无策略概念,永远能跑;
  - **编码**:PowerShell 5.1 中文系统默认 GBK 输出需额外设置(底座 #982 先例);cmd 用 `chcp 65001` 一行解决;
  - **版本漂移**:pwsh 7 vs 5.1 语法差异;cmd 在所有 Windows 行为一致;
  - **可预测性**:`cmd /C` 无包装语义,行为最确定;且无 bash 机器 fallback cmd = 现状,行为零变化。
  pinvou3 运行时 payload 无 bash,待 code 会话场景成熟再评估。

**状态**:① ✅ 已落地 + 已验证;② ⏳ 暂不实施

**① 验证记录**(2026-08-11,deepseek-v4-flash 实测,中性 prompt 三轮 A/B 共 6 次请求):

| 轮次 | A(无方言段) | B(带方言段) |
|---|---|---|
| 1 | `find \| xargs wc \| tail`(bash) | `findstr /R /N "^" \| find /C ":"`(cmd) |
| 2 | `find -exec cat \| wc`(bash) | `for /r %u in (*.ts *.tsx) ... %TEMP%\%RANDOM%`(cmd) |
| 3 | `find \| xargs wc \| tail`(bash) | `findstr \| find /C`(cmd) |

结论:同一模型同一问题,有无方言段生成完全不同的 shell 方言——A 恒 bash(3/3,cmd 下必失败)、B 恒 cmd(3/3,可执行且用地道 cmd 语法 for /r / %VAR% / &)。**方案①有效且稳定**。生产弱模型(Qwen3.6)待 vLLM 启动后补测。

---

## 解决记录

| 日期 | 问题 | 动作 | 结果 |
|---|---|---|---|
| 2026-08-11 | #1 | 方案①落地(命令规范,commit e5777588);方案②定案暂不实施 | ✅ ①已落地,②待评估 |
| 2026-08-11 | #1 | 方案①真实验证:中性 prompt 三轮 A/B,无方言段恒 bash、带方言段恒 cmd | ✅ 验证通过(Qwen3.6 待补测) |

---

## 追加规范

**新增问题**:按 #1 模板追加一节(编号递增),固定五段:
`现象` → `根因`(分层:执行环境/语法/模型侧) → `方案`(各方案标注:✅ 已落地 / ⏳ 暂不实施,含文案) → `状态`

**状态词**(统一):⏳ 待落地 / 🔍 观察中 / ✅ 已落地 / 🚫 放弃

**原则**:一行一个事实;方案随问题走(不另设方案池);根因分层描述;简洁优先。
