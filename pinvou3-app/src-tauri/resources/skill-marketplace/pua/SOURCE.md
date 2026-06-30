# Vendored skill 来源

- 上游：https://github.com/tanweai/pua （子目录 `skills/pua/`）
- License：MIT（见 SKILL.md frontmatter `license: MIT`）
- 抓取日期：2026-06-22
- 内容：仅取主 skill（SKILL.md + references/），未改内容。
- 排除：上游 monorepo 其余 45 个 SKILL.md 变体（pua-en/pua-ja/各 AI 工具适配版）+ 根 plugin.json + 站点资源图片。

> 上游是 Claude Code plugin bundle（46 个 SKILL.md + plugin.json），底座 `/skill install` 因 ClaudePluginBundle 拒装；此处只 vendored 主 skill 供 pinvou3 技能市场离线安装。
