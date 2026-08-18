# 企业微信域技能来源声明

本目录下的 `wecomcli-*` 技能文档来自腾讯官方 **wecom-cli** 项目,按 **MIT 许可证**分发。

- 上游仓库:https://github.com/WecomTeam/wecom-cli (`skills/` 目录)
- 许可证:MIT(随上游)
- 用途:pinvou3 接入企业微信(`@wecom/cli`)连接器时,作为官方域技能由引擎
  `SkillRegistry` 渐进披露,教模型用 `wecom-cli <域> ...`。

> 该 NOTICE.md 本身不含 `SKILL.md`,不会被技能注册表当作技能加载(与 lark NOTICE 同)。

## MIT License

以下许可证文本取自上游仓库 `LICENSE` 文件(https://github.com/WecomTeam/wecom-cli/blob/main/LICENSE):

```
MIT License

Copyright (c) 2026 WeCom

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Pinvou3 本地修改登记

以下修改为 pinvou3 在上游 skill 基础上的本地分叉。**本地修改在下次上游 sync 时需重放。**

### 上游同步(2026-08-18,wecom-cli 1.1.0)

全量同步至上游 v1.1.0 发布提交 `cd0480e0`(npm `@wecom/cli@1.1.0` 同源)。
1.1.0 上游重构了命令模型(鉴权 `init`→`auth init`、`msg`→`message`、
`schedule`→`calendar`、入参 JSON 位置参数→命名 flags/`--json`/`--set`,
新增 chat/disk/doc-manage/email/media/sheet/smartpage/shared 服务技能),
上游 `skills/` 目录随之重排为 14 个技能,本地结构**整体跟随上游**,不再维持
0.1.9 时代的「sheet/smartpage 并入 wecomcli-doc」合并形态(该合并的结构性
前提——doc 服务吞并 sheet/smartpage——已随上游服务模型拆分而消失)。
旧目录 `wecomcli-msg`/`wecomcli-schedule` 从包内移除,存量用户解包目录由
`apply_wecom_skills` 门控清理(BUNDLE_VERSION 0.23)。

### 0.1.9 时代历史登记(已在 1.1.0 同步中自然吸收或失效,存档备查)

- **结构性修改(失效)**:wecomcli-doc 合并 sheet/smartpage——1.1.0 上游已按
  服务模型独立拆分,合并形态废弃。
- **提示词去重与精简(2026-07-25)(吸收)**:0.1.9 时代的 contact/doc/msg/
  schedule 四技能去重精简与 meeting 结构下沉,随旧目录移除;1.1.0 上游重写了
  全部技能文档,新一轮去重以 1.1.0 文本为基线另行评估。
- **上游同步(2026-07-25)(失效)**:todo 同步 `9d2aeaf`、smartsheet 同步
  `bae1cc3e`——两者的内容已包含在 1.1.0 重写版中。
- **真实性审查(2026-08-16)(吸收)**:0.1.9 二进制命令面核实与 frontmatter
  防误用前缀(「何时用:」开头、≤280 字符)作为品悟常设口径保留,本轮已在
  全部 14 个技能上重放;`File(action="read")` 引擎工具名口径、smartsheet
  `records.values` 双层嵌套 JSON 上游 bug 修正,在 1.1.0 新文本上复核重放。

### 本轮(1.1.0)品悟适配清单

1. **frontmatter description 防误用前缀**:14 个 SKILL.md 的 description 改写为
   「何时用:仅当用户明确指向企业微信…时使用;泛指…默认走本地工具」开头,全部
   ≤280 字符(上游 disk/doc/smartsheet 三个超限 description 压缩改写)。
2. **安装教学改品悟代管口径**:wecomcli-shared Step 1 的
   `npm install -g @wecom/cli` 自更新指引(会绕过品悟 lock 钉扎触发哈希不匹配
   重装循环)改写为「wecom-cli 由品悟代管、随应用更新;版本不足时在工具商店
   企业微信卡片重新点连接触发安装/升级」。
3. **引擎工具名口径**:上游「先 `read` 对应 references 文件」类裸 read 表述改为
   「先用 `File(action="read")` 读取对应 references 文件」(smartpage/sheet/
   smartsheet/calendar/doc-manage/meeting 及其 references)。
4. **python3 口径**:doc-create 等 references 的 `python build_docx.py` 示例统一
   `python3`(宿主环境无裸 python)。
5. **上游 bug 修正重放**:smartsheet `records.values` 双层嵌套 JSON 结构修正
   (若 1.1.0 上游原文仍为 `"values": {"values":{...}}` 则保持单层修正版)。

### 各技能重放基线

14 个技能全部 = 上游 `cd0480e0`(v1.1.0 发布提交,npm 1.1.0 同源),技能目录与
上游同名同构;本地分叉仅为上文「本轮品悟适配清单」五类。

> 对账命令(仓库根执行):
> ```
> git clone https://github.com/WecomTeam/wecom-cli.git /tmp/wecom-upstream
> git -C /tmp/wecom-upstream worktree add /tmp/wecom-verify cd0480e0
> diff -rq /tmp/wecom-verify/skills/<skill> \
>   pinvou3-app/src-tauri/resources/common/bundle/wecom-skills/<skill>
> ```
> diff 只应报 SKILL.md(及被改的 references/*.md)differ;「Only in」之外的
> 差异即为新的未登记分叉。
