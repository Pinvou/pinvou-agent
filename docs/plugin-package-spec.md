# 插件包设计规范（Plugin Package Spec）

> 面向**包作者**的落地规范：定义一个可以被 pinvou 应用商店统一导入、识别、
> 落盘、开关与卸载的**标准插件包**该长什么样。
>
> 本文是 `plugin-protocol.md`（协议设计草案）里 **v1 已实施子集**的权威落地版：
> 以当前 `features/marketplace/plugin_import.rs`、`bundle.rs`、`mod.rs` 的实际实现为准。
> 协议草案中标注 v2 / 未实施的 `workflows`、`cli_connectors`、`commands`/`hooks`
> 不在本文范围内。
>
> 配套工具：预置市场技能「插件包标准化」`package-author`
> （`resources/common/skill-marketplace/package-author/`，工具商店「技能」区安装）
> 可在会话中指导大模型把任意用户文件（技能 / MCP / 组合）标准化成符合本规范的包。

---

## 1. 定位与不变量

- 一个插件包 = 一张卡 = 一个开关 = 一个落盘目录 `bundles/<id>/`。
- 包类型（`kind`）由**解包后的内容现算**，`plugin.json` 里没有自报类型字段；
  声明与实际不符（声明了 skill 却缺 `SKILL.md`）在导入时即失败。
- 空包（没有任何组件）在 schema 层拒收。
- 凭据永不落盘：只进系统 keyring，`mcp.json` 只写 `${PINVOU3_MCP_SECRET_*}` 占位符。

---

## 2. 包结构（zip 布局）

```
my-plugin.zip
├── plugin.json                 ← 权威声明（组合包/凭据/元数据/多组件时必需；
│                                 纯单组件可省略，走结构回退，见 §5）
├── mcp/                        ← MCP server（manifest.json + 服务脚本）
│   ├── manifest.json
│   └── server.py
├── skills/<name>/              ← SKILL.md 目录（可多个）
│   ├── SKILL.md
│   └── references/…            ← 可选，渐进披露用的参考文档
└── icon.svg | icon.png         ← 可选图标（缺省自动生成默认图标）
```

要点：

1. 组件按目录约定分域（`mcp/`、`skills/`），与落盘后的
   `bundles/<id>/` 布局同构，导入时映射直搬。
2. **一个包一个 MCP server**：MCP 组件的权威布局是**扁平 `mcp/`**（`mcp/manifest.json`
   + 脚本），`plugin.json` 里 `components.mcp_servers[].dir` 写 `"mcp"`。运行层
   `available_tools`/`load_manifest` 只读 `bundles/<id>/mcp/manifest.json`。
3. 裸包兼容：没有 `plugin.json` 的 zip 走结构回退（§5），导入时补生成规范化清单。

---

## 3. 插件清单 `plugin.json`（schema v1）

`manifest_version` 是协议版本（当前 = 1）；`version` 是插件自身的语义版本。

```jsonc
{
  "manifest_version": 1,                // 必填，=1
  "id": "weather-insight",              // 必填，包 id：[a-z0-9-_]{1,64}
  "name": "天气洞察",                    // 必填，展示名
  "version": "1.2.0",                   // 可选，语义版本
  "description": "聚合天气查询与解读",     // 可选，功能事实
  "icon": "icon.svg",                   // 可选，相对 zip 根（icon.svg / icon.png）

  // 多组件声明（mcp_servers / skills）
  "components": {
    "mcp_servers": [
      { "id": "weather", "dir": "mcp" }
    ],
    "skills": [
      { "id": "weather-interpret", "dir": "skills/weather-interpret" }
    ]
  }
}
```

字段语义：

| 字段 | 必填 | 说明 |
|---|---|---|
| `manifest_version` | ✅ | 协议版本，当前必须 = 1；高于支持版本时拒装并提示升级 |
| `id` | ✅ | 包 id，落盘目录名，`[a-z0-9-_]{1,64}` |
| `name` | ✅ | 展示名（任意可读文本） |
| `version` | 可选 | 语义版本。**预留字段**：当前仅解析保存，无消费方（不缺省合成、不参与升级判定） |
| `description` | 可选 | 功能事实。**预留字段**：当前仅解析保存，无消费方（卡片副标题来自商店数据/组件 manifest，不读本字段） |
| `icon` | 可选 | 图标文件，相对 zip 根，仅 `icon.svg`/`icon.png` |
| `components` | 可选 | 多组件声明，见下 |

`components` 两个子表（当前版本仅这两类）：

- `components.mcp_servers[]`：`{ id, dir }`，`dir` 相对 zip 根、导入时校验目录存在。
- `components.skills[]`：`{ id, dir }`，`dir` 相对 zip 根、导入时校验 `dir/SKILL.md` 存在，
  `id` 必须等于该 `SKILL.md` frontmatter 的 `name`。

前向兼容：解析**不用** `deny_unknown_fields`，未知字段原样保留（flatten），
方便未来加 `credentials`/`config_fields`/`dependencies` 等不破坏旧包。

---

## 4. 组件类型与 `kind` 推导

`kind` 现算（`derive_bundle_kind`），优先级自上而下：

| 组件向量 | `kind` |
|---|---|
| mcp 非空 && skills 非空 | `Bundle`（组合包） |
| 仅 mcp 非空 | `Mcp`（纯 MCP） |
| 仅 skills 非空 | `Skill`（纯技能） |
| 全空 | 拒收（空包） |

（旧 `Spanner` 变体已移除；可执行能力方向是 SKILL.md frontmatter `tools[]` + `runtime`
段声明 + skill-run wrapper——该方向为 **RFC 草案，执行通路未实施**，本文不展开。）

（内置 `Cli` 连接器不走插件包上传，v1 不开放。）

---

## 5. 无 `plugin.json` 的结构回退（裸包）

没有 `plugin.json` 时按目录结构识别，并落盘一份派生的规范化 `plugin.json`：

- **裸 MCP**：`mcp/manifest.json`（标准布局，只要求能解析且 `id` 非空），或 zip 内
  **任意其他位置**的 `manifest.json` 能解析出 `ToolManifest`、`id` 非空且声明了
  `command` 或 `servers` → 识别为纯 MCP，规范化为 `mcp/`。
- **裸技能**：`skills/<name>/SKILL.md`，或任意位置 `SKILL.md`（frontmatter `name`
  合法）→ 识别为纯技能，规范化为 `skills/<name>/`。

---

## 6. MCP 组件：`mcp/manifest.json`（`ToolManifest`）

MCP server 的启动真相源。本地 stdio server 的必填与常用字段：

```jsonc
{
  "id": "weather",              // 必填，[a-z0-9-_]{1,64}（与包 id / components 声明一致）
  "name": "天气查询",             // 必填
  "description": "查指定城市天气", // 必填
  "version": "1.0.0",           // 必填
  "icon": "",                   // 必填（可为空串；已装工具图标走包级 bundles/<id>/icon.*）
  "category": "life",           // 必填，分组用（自由串，建议 dev/office/life/data/search 等 slug）
  "mcp_tools": [],              // 必填（本地 stdio 可为空：工具由 server 运行期 tools/list 声明）
  "command": "python",          // 必填（本地：解释器命令）
  "args": ["server.py"]         // 必填（本地：入口脚本名，见下）
}
```

本地 stdio 约定：

- `command` = 解释器（`python` / `node` / …）。
- `args` = 入口脚本，**脚本必须命名为 `server.py`** 且 `args: ["server.py"]`
  （安装时仅 `server.py`（或以 `/server.py` 结尾的参数）会被重写为 `bundles/<id>/mcp/server.py`
  绝对路径；其他脚本名原样写入 mcp.json，无法定位安装目录）。
- 依赖 pip 包 → 声明 `"pip_dependencies": ["requests"]`。注意：上传/导入路径
  **不会自动安装**（供应链安全，仅日志提示需用户自行 `pip install`）；只有内置
  商店工具的安装管线才会自动装。
- 敏感项走 `secret_env` / `secret_headers` / `config_fields`（`secret: true`），
  值只进 keyring，落盘占位符。

远程 server（OAuth/HTTP）：用 `servers` 取代 `command`/`args`：

```jsonc
{
  "id": "qcc", "name": "企查查", "description": "d", "version": "1.0.0",
  "icon": "", "category": "search", "mcp_tools": [], "command": "", "args": [],
  "servers": [
    { "name": "qcc", "url": "https://…", "oauth": { "client_id": "…" } }
  ]
}
```

可选字段全量：`env`、`secret_env`、`secret_headers`、`validate_on_install`、
`config_fields`、`routing_rules`、`tool_table_entries`、`pip_dependencies`、
`servers`、`companion_skills`。

---

## 7. ~~spanner 组件（扳手插件）~~（已移除，历史存档）

> 2026-08：spanner 独立组件模型已移除。脚本可执行能力的演进方向是 skill 包
> SKILL.md frontmatter `tools[]` + `runtime` 段 + `skill-run` wrapper——
> **该方向为 RFC 草案，执行通路未实施**（完整历史设计见 `plugin-protocol.md` §15 存档）。旧包中的 `spanner` 字段
> 经 plugin.json `extra` 保留、deser 不炸，但不再合成 mcp/manifest.json。

介于「脚本 skill」与「MCP」之间：声明式 schema + 无状态单次进程 + 自带运行时。

```jsonc
{
  "manifest_version": 1,
  "id": "weather",
  "name": "天气查询",
  "version": "1.0.0",
  "description": "查城市天气",
  "icon": "icon.svg",
  "spanner": {
    "entry": "main.py",                          // 入口，相对 spanner/（也可写 "spanner/main.py"）
    "runtime": { "kind": "python", "dir": "runtime" }, // 可选；缺省用内置 python
    "input_schema":  { "type": "object", "properties": { "city": { "type": "string" } }, "required": ["city"] },
    "output_schema": { "type": "object" },       // 可选
    "timeout_secs": 30,                          // 可选
    "background": false                           // 可选
  }
}
```

调用约定（作者视角）：引擎 `spawn <runtime> main.py`，参数 JSON 写 stdin，结果 JSON
写 stdout。作者只写：

```python
import json, sys
args = json.load(sys.stdin)
result = do_work(args)
json.dump(result, sys.stdout)
```

安装时由内置 `spanner_runner` 包装成 MCP 工具（自动合成 `mcp/manifest.json` 并设
`spanner_entry`），无需作者理解 MCP 协议。`input_schema` 必填；安装前会做一次真实
spawn 自检（喂空参数、校验 stdout 合法 JSON），跑不通即拒装。

---

## 8. 技能组件：`SKILL.md`

技能目录内必须有 `SKILL.md`，frontmatter 至少两个字段：

```markdown
---
name: weather-interpret        # 必填，[a-zA-Z0-9_-]{1,64}（允许大小写）
description: 解读天气数据……     # 必填（展示 + 触发判定的依据；何时使用写清楚）
metadata:                       # 可选
  type: user
---
# 正文：给模型看的指令
```

- `name` 与 `components.skills[].id` 必须一致。
- `description` 建议一句话讲清「做什么 + 什么时候用」，是模型决定是否调用它的关键。
- 子资源放 `references/`（渐进披露：正文只给目录，需要时再 `load_skill` 读）。

---

## 9. 图标规范

- 可选：zip 根放 `icon.svg` 或 `icon.png`（仅这两种扩展名），`plugin.json.icon` 引用。
- 缺省：无图标 → 导入时落盘内置默认图标 `bundles/<id>/icon.svg`。
- 落盘后图标与工具同目录：`bundles/<id>/icon.<ext>`；前端「已装工具」一律读这里。

---

## 10. 命名安全（id 校验）

| 用途 | 规则 |
|---|---|
| 包 `id`（`plugin.json.id`） | `[a-z0-9-_]{1,64}`，小写 |
| MCP `id`（`ToolManifest.id`）与 `components.mcp_servers[].id` | `[a-z0-9-_]{1,64}`，小写 |
| 技能 `name`（`SKILL.md`）与 `components.skills[].id` | `[a-zA-Z0-9_-]{1,64}`（允许大小写；两者必须一致，导入仅校验一致性） |

禁止 `.`、`..`、路径分隔符；与已下线内置名冲突会拒收。

---

## 11. 导入校验（安全边界）

导入时统一预检，任一不过即拒收、不留半安装态：

1. 路径穿越（`enclosed_name()` + 写出前二次断言）。
2. symlink / hardlink（`unix_mode` 高 4 位 = `0o120000`）。
3. 体积：解压累计 ≤ 200 MiB；zip 头声明与实际解压大小不符（伪造头/zip bomb）拒收。
4. 名字安全（§10）。
5. 组件声明与实际一致：
   - 声明的 `dir` 必须存在；skill 目录必须有 `SKILL.md`；
   - MCP 组件 `dir` 必须精确为 `mcp`（其他值拒收），且组件 `id` 与 `mcp/manifest.json` 的 `id` 交叉一致；
   - 未声明或未被识别的 `skills/<other>/` 子树整体拒收（防止不可见孤儿技能）。
6. 凭据不落盘（§6）。
7. `manifest_version` 高于支持版本时拒装（§3）。
8. 命名冲突：与预置 MCP（`mcp_catalog`）、内置 CLI 技能目录或已下线技能名冲突时拒收。
9. **同 id 包更新语义**：已存在的包 id 仅允许同内容重导（视为原子替换）；内容不同的同名包拒收——更新包内容需更换包 id，或先手动删除 `~/.pinvou3/bundles/<id>/` 目录再导入（Upload 来源包卸载时保留该目录作为用户唯一副本，"先卸载再导入"清不掉同 id 冲突）。

> 导入成功 ≠ 立即可用：出于安全默认，上传包安装后 code 模式默认禁用，需用户在能力开关中显式开启。

---

## 12. 示例

### 12.1 纯 MCP 包

```
calc.zip
├── plugin.json
├── mcp/
│   ├── manifest.json
│   └── server.py
└── icon.svg
```

`plugin.json`：`components.mcp_servers: [{ "id": "calc", "dir": "mcp" }]`；
`mcp/manifest.json`：`id=calc, command=python, args=["server.py"]`。

### 12.2 纯技能包

```
greet.zip
├── plugin.json
└── skills/greet/
    └── SKILL.md          # name: greet
```

`plugin.json`：`components.skills: [{ "id": "greet", "dir": "skills/greet" }]`。
（也可省略 `plugin.json`，走裸技能回退。）

### 12.3 组合包（MCP + 技能）

```
combo-demo.zip
├── plugin.json
├── mcp/
│   ├── manifest.json      # companion_skills: ["combo-demo"]
│   └── server.py
└── skills/combo-demo/
    └── SKILL.md           # name: combo-demo
```

`plugin.json` 同时声明 `mcp_servers` 与 `skills`；`mcp/manifest.json` 的
`companion_skills` 指向技能，让「引擎 + 使用引导」同卡、同开关、整体装卸。

---

## 13. 与其它文档的关系

- `plugin-protocol.md`：协议**设计草案**，含 v2 预留（workflows / cli_connectors /
  commands / hooks / 远程下载等）。本文只落地其 v1 已实施子集。
- `capability-governance.md` / `marketplace-unification.md`：能力包统一模型与治理
  （一个包 = 一张卡 = 一个开关；kind 现算；禁用保证在执行层）的上游依据。
- 本规范由预置市场技能「插件包标准化」`package-author` 落地为可执行指令（见其 `SKILL.md`）。
