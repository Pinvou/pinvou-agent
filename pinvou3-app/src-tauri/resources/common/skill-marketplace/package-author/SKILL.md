---
name: package-author
description: 当用户要把手头的工具打包/标准化成 pinvou 插件包时使用——包括纯技能(SKILL.md)、纯 MCP 服务或它们的组合包。用户说"打包/做成插件包/标准化这个工具/给我一个能上传的标准包/写 plugin.json/加个图标"等，或给了散乱脚本/目录要整理成可上传 zip 时，用本技能把内容规范成 plugin-protocol 标准包（补 plugin.json、补 mcp/manifest.json、补 SKILL.md、补图标、校验命名）。
---

# 插件包标准化（package-author）

把用户给的工具/技能/脚本，整理成 pinvou 应用商店可导入的**标准插件包**。规范以
`docs/plugin-package-spec.md` 为准，本文件内置完整规则，可直接照做、不依赖文档可达。

## 何时用 / 何时不用
- ✅ 用：用户要打包/标准化一个工具，给了文件/目录/脚本/SKILL.md/manifest.json/zip，或口头描述要一个插件包。
- ❌ 不用：用户只是"写个脚本"/"写个技能内容"而没说要打包；或是在装/卸载/开关某个已存在的工具。

## 先弄清三件事（问清再动手，别猜）
1. **输入在哪**：用户给的是目录路径、粘贴的代码、还是 zip？文件类型是什么？
2. **包类型**：纯技能？纯 MCP？还是 MCP+技能组合？（见 §类型判定）
3. **输出形态**：要一个**目录**（可直接 zip），还是直接产出 **zip**？

若用户没给 id/名称，按内容起一个语义化小写 id（如 `weather-insight`），并告诉用户可改。

---

## 类型判定（决定布局）

| 用户给的东西 | 类型 | 标准布局 |
|---|---|---|
| 只有 SKILL.md 或技能目录 | 纯技能 Skill | `skills/<name>/SKILL.md` |
| 一个 MCP server（脚本 + 描述） | 纯 MCP Mcp | `mcp/manifest.json` + `mcp/server.py` |
| MCP + 配套使用引导技能 | 组合 Bundle | `mcp/` + `skills/<name>/` 同时存在 |

---

## 标准包结构（必须落成这样）

```
<id>/
├── plugin.json                 ← 权威声明（见下）
├── mcp/                        ← 纯 MCP / 组合包才有
│   ├── manifest.json
│   └── server.py
├── skills/<name>/              ← 纯技能 / 组合包才有
│   └── SKILL.md
└── icon.svg | icon.png         ← 图标（缺失则生成，见 §图标）
```

---

## plugin.json（schema v1）

```jsonc
{
  "manifest_version": 1,                // 必填，=1
  "id": "weather-insight",              // 必填，[a-z0-9-_]{1,64} 全小写
  "name": "天气洞察",                    // 必填，展示名
  "version": "1.0.0",                   // 可选
  "description": "聚合天气查询与解读",     // 可选
  "icon": "icon.svg",                   // 可选，相对根，icon.svg/icon.png
  "components": {                       // 多组件用
    "mcp_servers": [ { "id": "weather", "dir": "mcp" } ],
    "skills":      [ { "id": "weather-interpret", "dir": "skills/weather-interpret" } ]
  }
}
```

硬规则：
- `id` 全小写 `[a-z0-9-_]`，禁 `.`/`..`/路径分隔符；`name` 可任意可读文本。
- `components.mcp_servers[].dir` **写 `"mcp"`**（扁平单 server）；skills 的 `dir` 写
  `"skills/<name>"`，且 `<name>` 必须等于该 SKILL.md frontmatter 的 `name`。
- 纯单组件可省略 `plugin.json`（导入走结构回退），但**标准化输出一律补上**（自描述）。
- 未知字段别乱加；当前只认 `components{mcp_servers,skills}`。

---

## MCP 组件：mcp/manifest.json

本地 stdio server（最常见）：

```jsonc
{
  "id": "weather",              // [a-z0-9-_]{1,64}，与 plugin.json 声明一致
  "name": "天气查询",
  "description": "查指定城市天气",
  "version": "1.0.0",
  "icon": "",                   // 空串即可（图标走包级 icon.*）
  "category": "life",           // 分组 slug：dev/office/life/data/search…
  "mcp_tools": [],              // 本地可为空：工具由 server 运行期 tools/list 声明
  "command": "python",          // 解释器：python / node …
  "args": ["server.py"]         // 入口脚本名（推荐就叫 server.py）
}
```

硬规则：
- 入口脚本**命名为 `server.py`**、`args: ["server.py"]`（安装时被重写为包内绝对路径）。
- 依赖 pip 包 → `"pip_dependencies": ["requests"]`。
- 密钥/Token **不写明文**：走 `config_fields`（`secret:true`）或 `secret_env`/`secret_headers`。
- 远程 HTTP/OAuth server：用 `servers:[{name,url,oauth?}]` 取代 `command`/`args`。

---

## 技能组件：SKILL.md

```markdown
---
name: weather-interpret        # [a-zA-Z0-9_-]{1,64}（可大小写）
description: 解读天气数据，用户要分析/解释天气结果时使用。  # 必填：做什么 + 何时用
---
# 正文：给模型看的指令
```

- 若用户给的技能缺 frontmatter 或缺 `description`，**补上**（description 是触发依据）。
- 子资料放 `references/`，正文只列目录。

---

## 图标

- 用户没给图标 → **生成一个 `icon.svg`**（简约扁平、单色几何图形，24×24 viewBox、
  `stroke="currentColor"`、无外链/无脚本），并在 `plugin.json` 写 `"icon": "icon.svg"`。
- 只认 `.svg` / `.png`。用户给了图就校验扩展名，不合规则转 SVG 或换名。
- 禁止外链图片、禁止脚本、禁止超长/畸形 SVG。

---

## 校验清单（交活前逐条过）

1. `plugin.json` 存在且 `manifest_version:1`、`id` 全小写合法。
2. 目录结构符合 §标准包结构；声明了 `components` 的 `dir` 都在包内、skill 目录有 `SKILL.md`。
3. MCP：`mcp/manifest.json` 的 `id/name/description/version/icon/category/mcp_tools/command/args` 八项齐（可空值但字段在），`command` 是解释器、`args` 指向 `server.py`。
4. 技能：`SKILL.md` frontmatter 有 `name` + `description`。
5. 图标：有 `icon.svg` 或 `icon.png`。
6. 无明文密钥/Token；无路径穿越名（`.`/`..`/分隔符）。

## 产出

- 默认把标准化结果**写成一个目录**（相对工作目录，如 `<id>/…`），并列出文件清单。
- 用户要 zip → 用可用的打包方式压成 `<id>.zip`（根就是 `plugin.json`，别多套一层目录）。
- 收尾：一句话说明类型（纯技能/纯 MCP/组合）+ 落盘路径 + 如何在商店上传。
