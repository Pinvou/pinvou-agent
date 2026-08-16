# 第三方组件声明 — 腾讯会议官方技能(tmeet-skill)

本目录下的 `tmeet-skill/`(SKILL.md + references/)同步自腾讯会议官方开源仓库
**TencentCloud/tencentmeeting-cli**(https://github.com/TencentCloud/tencentmeeting-cli)
tag **v1.0.15** 的 `skills/tmeet-skill/`,按其 **MIT License** 分发。

上游仓库根目录 `LICENSE` 为腾讯版权声明的 MIT 许可;`skills/tmeet-skill/` 目录内
无单独 LICENSE 文件,故此处内联保留许可证文本:

```
Tencent is pleased to support the open source community by making tencentmeeting-cli available.

Copyright (C) 2026 Tencent.  All rights reserved.

tencentmeeting-cli is licensed under the MIT.


Terms of the MIT:
--------------------------------------------------------------------
Permission is hereby granted, free of charge, to any person obtaining
a copy of this software and associated documentation files (the
"Software"), to deal in the Software without restriction, including
without limitation the rights to use, copy, modify, merge, publish,
distribute, sublicense, and/or sell copies of the Software, and to
permit persons to whom the Software is furnished to do so, subject to
the following conditions:

The above copyright notice and this permission notice shall be
included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT,
TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE
SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
```

说明:

- 技能本体不随 npm 包分发(`@tencentcloud/tmeet` 包内不含 skills,这是常见踩坑:
  不要从 npm tgz 里找 skills),更新方式为按上游对应 tag 同步
  `skills/tmeet-skill/` 到本目录,保留本声明。具体操作:上游仓库
  TencentCloud/tencentmeeting-cli 的 tag 带 `v` 前缀,与品悟钉扎版本对应
  (当前 v1.0.15),取
  `https://github.com/TencentCloud/tencentmeeting-cli/archive/refs/tags/v1.0.15.tar.gz`
  或 `git clone && git checkout v1.0.15` 后,以其中 `skills/tmeet-skill/`
  整目录为三方合并基线,再按下文登记逐条重放。
- 品悟按用户连接状态门控该 skill:仅在用户已连接 `tmeet` 且未禁用腾讯会议技能时
  释放到运行时技能目录。
- `tmeet` CLI(`@tencentcloud/tmeet`)不随包内置,由
  `pinvou3-app/src-tauri/src/features/connectors/tmeet.rs` 的 npm 钉扎
  (`TMEET_NPM_SPEC`,当前 `@tencentcloud/tmeet@1.0.15`)在线安装;SKILL.md 的
  「安装与初始化」节已按下方登记第 4 条改写为品悟代管口径,上游的
  `npm install -g @tencentcloud/tmeet@latest` 教学不再出现在技能正文,实际版本
  以 Rust 层钉扎为准。

## Pinvou3 本地修改登记

技能文档命令树与参数均已对照 tmeet 1.0.15 实测 help 核验（含 1.0.15 新增的
`meeting search`、`control waiting-room`），无发现不符。为适配品悟运行形态，在
上游 tag v1.0.15 的 `skills/tmeet-skill/` 基础上做了以下修改（1-4、6 的 SKILL.md
部分仅限 `SKILL.md`；第 5 条与第 6 条另涉 `references/tmeet-record.md` /
`references/tmeet-auth.md` 各一处措辞修正，其余 references/ 与上游逐字节一致，
已经上游 tag diff 逐文件复核。第六轮核验注 2026-08-16：上述「其余 references
逐字节一致」经 /tmp 上游 v1.0.15 快照重验仍成立——tmeet-meeting/contact/
tshoot/report/control 五篇与上游一致，record.md / auth.md 的差异即第 5、6 条
所述内容）：

1. **frontmatter `description` 重写**：上游 description 长 327 字符，超过品悟
   SkillRegistry 的 280 字符截断上限，压缩为 211 字符，并按品悟契约改为
   「何时用：」开头、附「泛指需求默认走本地工具」防误用语义。
2. **读取工具名适配**：SKILL.md「录制查询」节 CRITICAL 前置块中的上游写法
   「MUST 先用 Read 工具读取 `references/tmeet-record.md`」改为「MUST 先用
   `File(action="read")` 工具读取」（CodeWhale canonical 工具族命名；全包实测
   仅此 1 处，tmeet-auth.md 等其余文件无读取工具指引）。
3. **悬空占位链接修复**：命令总览说明行中的占位示例
   `[references/xxx.md](references/xxx.md)` 改为纯代码格式 `references/xxx.md`
   （原写法是指向不存在文件的悬空 markdown 链接，仅去链接化，语义不变）。
4. **「安装与初始化」节改写为品悟代管口径**：上游的 `npm install -g
   @tencentcloud/tmeet@latest` 自动安装指引改为「由品悟应用代为安装与管理，
   模型不要自行执行安装命令」，并移除文档内的具体安装命令，避免模型在品悟
   内自行安装/升级。
5. **登录前置例外清单修正**（references 同步一处措辞修正）：按 1.0.15 源码
   `cmd/root.go` preCheck 与 `cmd/tshoot/log.go` 的 `skipPreCheckFlag("upload")`
   实测，免登录命令除 `auth login` / `auth status` 外还包括不带 `--upload` 的
   `tshoot log`，SKILL.md 认证节例外清单已补齐；`references/tmeet-record.md`
   中「唯一能按录制内容检索的命令」补「跨会议」限定，消除与
   `transcript-search` 的表述矛盾。
6. **宿主环境断言改为品悟口径**（2026-08-16 第四轮审查，SKILL.md 与
   `references/tmeet-auth.md` 各两处）：上游「如果当前 Agent 是 Hermes 且系统
   没有默认浏览器」改为「品悟运行环境始终可访问默认浏览器（登录走
   `--no-browser`），仅品悟之外的 Agent 环境保留原 Hermes 提示」；上游「第一次
   调用 auth login 必须将 agent 类型/模型名写入 `TMEET_AGENT`/`TMEET_MODEL`」
   改为「两变量由品悟宿主统一注入（`tmeet.rs` 固定 `Pinvou`），模型不要自行
   设置，仅品悟之外环境才自行写入」。两变量经 strings 实测存在于 1.0.15
   二进制、品悟注入值见
   `pinvou3-app/src-tauri/src/features/connectors/tmeet.rs`。

上游其余内容（含 `auth login` 交互式登录教学等）保持上游原样；品悟实际安装
版本由 `tmeet.rs` 的 `TMEET_NPM_SPEC` 钉扎（`@tencentcloud/tmeet@1.0.15`），
实际登录由 `auth login --no-browser` 完成（该 flag 在 1.0.15 help 中真实存在），
文档描述与品悟用法不矛盾。
