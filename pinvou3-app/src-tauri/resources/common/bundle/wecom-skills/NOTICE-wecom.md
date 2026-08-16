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

### 结构性修改

- **wecomcli-doc**:合并了上游独立的 `wecomcli-sheet`(表格)与 `wecomcli-smartpage`(智能文档)两个 skill 的能力(自上游 PR #67 起),统一按 URL 品类路由到 `get_doc_content` / `smartpage_*` 接口;普通表格仅支持读取,智能表格结构管理拆出为 `wecomcli-smartsheet` skill。

### 提示词去重与精简(2026-07-25)

- **wecomcli-contact**:返回字段表格压缩为一行;删除 errcode 重复说明、「快速参考」节与工作流 3(「只调一次 get_userlist」并入工作流 1 末尾注释);工作流 1 步骤 1 的重复代码块改为单行引用。
- **wecomcli-doc**:description 压缩至 280 字符截断上限内;删除 URL 表格下逐行复读的「判断规则」四条 bullet,保留一行 ⚠️ 提示;智能文档触发规则 3 处重复只保留开头一处;「典型工作流」由 JSON 示例复读压缩为三行决策清单。
- **wecomcli-msg**:删除与工作流步骤 8-9 重复的「强制交互步骤」节,改为步骤 8 前一行提示;`MEDIA:` 禁令(pinvou3 不存在该指令)改为正向表述;HTTP 错误重试表述改为「可重试,仍失败则展示错误信息」。
- **wecomcli-schedule**:同名候选规则 3 处重复保留 1 处,其余改为短引用;errcode/时间格式两条注意事项合并为一句;各工作流「经典 query 示例」每个保留 2 条。
- **四个 skill 共性**:删除每个文件开头的 `wecom-cli` 样板说明句(命令示例本身已表明用法)。

### 上游同步(2026-07-25)

- **wecomcli-todo**:全量同步至上游 `9d2aeaf3`(2026-07-02 todo API 重写:`remind_time` 删除、`get_todo_list` 必填 `follower_id`、新增 `search_todo_userid`);删除基于旧 API 的 `examples/` 目录;末尾追加「pinvou3 增补行为要求」三条(列表后必须查详情、分页必须提醒、删除/拒绝前必须确认)。
- **wecomcli-smartsheet**:全量同步至上游 `bae1cc3e`(新增新建智能表格、851002 错误码表、接口路由表);description 替换为 pinvou3 适配版(上游引用的 wecomcli-sheet/wecomcli-smartpage 在本包并入 wecomcli-doc)。

### 结构下沉(2026-07-25)

- **wecomcli-meeting**:工作流 3-7(查列表/详情/关键词/取消/成员更新,155 行)下沉至 `references/workflows.md`,本体留索引表;「注意事项」去重合并为 5 条。本体 475 → 323 行。

### 真实性审查(2026-08-16,wecom-cli 0.1.9 未升级)

对照上游 `WecomTeam/wecom-cli`(main = 0.1.9 发布提交 `72e14f7` + 后续 3 个 skill 提交,其中 `bae1cc3`/`9d2aeaf` 已同步、`9eb7898` 仅 README)与本机 0.1.9 二进制实测:

- **命令真实性核实**:用 0.1.9 二进制实测(向 `~/.config/wecom/cache/service_<域>.json` 注入上游 `registry.rs` 认可的工具清单后跑 `<域> <工具> --help`)全量验证 7 个 skill 引用的 45 条命令,全部存在;`get_doc_content`/`smartpage_*`/smartsheet 新建/todo 必填 `follower_id`/`search_todo_userid` 均与 0.1.9 相符,无需改动。
- **frontmatter 防误用语义补齐(pinvou3 适配)**:7 个 skill 的 description 原为上游直译,缺「何时用 + 泛指默认本地」防误用前缀(对照 lark/dws/tmeet 收录口径),逐个改写为 `何时用:仅当用户明确指向企业微信…时使用;泛指…默认走本地工具` 开头,全部 ≤280 字符。
- 未发现上游通用话术残留(文件读取工具名均为品悟口径、无全局安装类指引)、悬空相对链接(22 个引用全部存在)。
