钉钉内置技能来自钉钉官方 dingtalk-workspace-cli 的 dws-skills.zip mono 形态。

- npm package: dingtalk-workspace-cli
- bundled version: 1.0.51
- Linux ARM64 dws SHA-256: db012e54393ae0d1b78d74d0606e084823ab8e5540991deb6d31e68abd01883b
- license: Apache-2.0

Pinvou3 仅负责随应用内置并按用户连接状态门控该 skill。dws CLI 凭证由官方 CLI 管理。

## Pinvou3 本地修改登记

依据 Apache-2.0 §4(b) 登记对 `dws/SKILL.md` 的本地修改（2026-07-25）。下次升级 dws npm 版本时本节修改需重放。

1. frontmatter `description` 重写：修复「在线电子表格」重复出现、补入 AI应用与目标管理(Agoal)，并压缩为一句话触发说明。
2. 修正脚本能力描述：`scripts/` 下无 AI 应用创建轮询脚本，删除该说法（MUST DO「脚本优先」条与「详细参考」scripts 行两处）。
3. 修正「脚本均支持 `--dry-run` 预览、`--format json` 输出」的不实表述，改为提示各脚本参数不统一、先用 `--help` 确认 flag。
4. 产品总览表补 `agoal`（目标管理）行，与意图决策树已有路由对齐。
5. 压缩顶部警告块为一行（与「命令发现」节内容重复）。
6. `--yes` 确认规则去重：删除「确认流程」三步代码块与「命令发现」节末尾重复句，确认方式合并为「危险操作确认」节开头一句。
7. 「核心流程」删除元话术，压缩为 0-3 步（URL 预检/意图分类/歧义追问/选定产品读参考后执行）。
8. MUST DO 参数格式括号注压缩。
9. 「详细参考」中 best_practices 逐文件枚举压缩为单行汇总，aitable 两行合并为一行。
