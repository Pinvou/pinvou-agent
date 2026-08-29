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
| 包 `id`（`plugin.json.id`） | `[a-z0-9-_]{1,64}`，小写；导入强制（`is_safe_component_id`）。无 `plugin.json` 的裸包按回退识别取 id，改走 `is_safe_skill_name`（`[a-zA-Z0-9_-]{1,64}`，允许大小写） |
| MCP `id`（`ToolManifest.id`）与 `components.mcp_servers[].id` | 规范要求 `[a-zA-Z0-9_-]{1,64}`（建议小写）。**导入侧实际只强制**：两侧 id 一致、组件 `dir` 精确为 `mcp`；组件 id 字符集暂不校验（已知缺口，单独跟踪） |
| 技能 `name`（`SKILL.md`）与 `components.skills[].id` | 规范要求 `[a-zA-Z0-9_-]{1,64}`（允许大小写）且两者一致。**导入侧实际只强制**（清单路径）：组件 `dir` 精确为 `skills/<id>` 且其中存在 `SKILL.md`；frontmatter 不解析，name 与声明 id 的一致性及字符集均暂不校验（已知缺口，单独跟踪）。裸技能回退路径（无 `plugin.json`）取 frontmatter `name` 并校验字符集 |

禁止 `.`、`..`、路径分隔符；与已下线内置名冲突会拒收。

---

## 11. 导入校验（安全边界）

导入时统一预检，任一不过即拒收、不留半安装态：

1. 路径穿越（`enclosed_name()` + 写出前二次断言）。
2. symlink（`unix_mode` 文件类型位 `mode & 0o170000 == 0o120000` 即 S_IFLNK；zip 中 hardlink 表现为普通文件副本，无独立类型位，不在此拦截）。
3. 体积：解压累计 ≤ 200 MiB；zip 头声明与实际解压大小不符（伪造头/zip bomb）拒收。
4. 名字安全（§10；清单路径仅包 id 强制字符集，组件 id 校验缺口见 §10 表）。
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

## 12. 上传包的 UI 展示名/说明覆盖（应用侧登记，不改包清单）

用户可在插件中心给自己上传的包（登记来源为 Upload）设置可读的**显示名**与
**显示说明**，只影响 UI 展示：

- 覆盖值写在应用登记 `~/.pinvou3/marketplace/bundles.json` 对应记录的 `extra`
  map（key：`display_name` / `display_description`），**不改 `plugin.json`、不改包
  目录、不改 frontmatter `name`**；机读 id 与目录名保持不变。
- 展示优先级：extra 覆盖 > 包内现状（上传技能卡标题回退为上传文件名去扩展名、
  再退化为包 id，说明回退为 `SKILL.md` 的 `description`；上传 MCP/组合包回退为
  `mcp/manifest.json` 的 `name` / `description`；composer 工具菜单同样应用
  覆盖）。清空覆盖（留空保存）即删除 key、回退默认展示。
- 校验：显示名 ≤ 64 字符、显示说明 ≤ 240 字符，含控制字符/换行/不可见字符
  （bidi 控制、零宽、BOM、行/段分隔符等，可视觉欺骗或无法单行表达）一律拒绝；
  仅 Upload 来源可写（预置/内置包拒绝，手工塞进非 Upload 记录 extra 的覆盖在
  展示时也会被忽略）。
- **导入后即编辑**：上传导入成功（按钮选文件 / 拖放 zip / 拖放单 `.md`）立即
  打开展示信息编辑弹窗，显示名预填当前生效默认名（上述回退链的取值），用户可
  直接保存固定该名、改名后保存，或取消（不设覆盖，保留默认展示）。
- **SKILL.md 说明同步（双向）**：仅当包内有且仅有一个技能（`bundles/<id>/skills/`
  下恰一个技能目录）时：
  - 设置非空显示说明 → 首次回写前把 frontmatter 原说明备份进 `extra`
    （`skill_description_backup`，空串 = 原本没有 description），再把说明回写进
    该技能 `SKILL.md` 的 frontmatter `description`（统一写成单行；已有块状写法
    会被替换），让模型侧看到的描述与界面一致，并重算包内容指纹。一致性按会话
    生效时机分两档：**新会话**（及编辑后启动/重启的会话）在构建技能组合目录时
    取到回写后的值；**编辑时已在线的存量会话**的组合目录副本按「增删技能」
    增量维护，不比对已存在技能目录的内容变化，其模型侧描述要到该会话引擎下次
    全量重建（如重启会话）后才更新——这是会话技能组合机制的既有口径，非本
    编辑特性引入；
  - 清空显示说明 → 从备份恢复 frontmatter 原值（原缺失则删掉 description 行），
    然后清除备份 key。恢复后回到「从未编辑过」的状态。备份原值可能是多行
    （原文件块状写法）或含引号——恢复按原值无损回写：安全单行值写 plain、带
    首尾空白的单行值写成对单引号行、多行值写成块状字面量，恢复以备份值为准，
    会覆盖此期间对 `SKILL.md` description 的手工编辑。唯一无法表达的形态是
    **多行值的首个非空行带前导空白**（引擎块读取以首行定基准缩进，剥掉后无法
    还原）——此时清空报错（fail-closed，文件不动），展示层仍回退 frontmatter
    现值，不产生不一致状态。
  值无法单行互洽表达（含双引号、反斜杠）、或 frontmatter 存在结构性多重
  description 定义时整体报错、不落盘（备份 key 也不写）。两处既定限制：
  SKILL.md 带 BOM 或 `---` 围栏前有内容（前置空行等）的包，引擎可读但回写侧
  要求首行即 `---`，说明同步会整体报错（展示覆盖对非单技能形态仍可用）；
  frontmatter 缺 description 时说明回退展示为空，不受 240 字符展示截断影响
  （截断仅作用于从 frontmatter 读取的展示口径，备份/回写用原值）。多技能包与
  纯 MCP 包跳过同步（说明覆盖仅写入 extra，不备份）。重导入整体替换包内容时，
  说明备份随内容重基线（丢弃，防止把旧包描述恢复进新包；编辑与同 id 重导入
  经按包 id 的导入互斥锁串行化）——三条上传通道（按钮选文件 / 拖放 zip /
  拖放单 `.md`）的统一导入路径与遗留技能包导入路径均已覆盖；展示名/说明覆盖
  本身按上文语义跨重导入保留。
- **编辑与重导的交互**：说明回写改变了包内容，同一 zip 之后再导入会按 §11
  第 9 条的同 id 内容比对被判「内容不同」拒收——更新包内容需更换包 id，或先
  删除 `~/.pinvou3/bundles/<id>/` 目录再导入。能否「先卸载再导入」按包形态分：
  Upload 的 **MCP/组合包**卸载保留包目录作为用户唯一副本，清不掉同 id 冲突；
  Upload 的**纯技能包**卸载会删除包目录与登记，卸载后可直接重导同 id。
  已知限制：Upload 的 MCP/组合包卸载会删除 bundles.json 登记（展示覆盖与说明
  备份随记录一并清除，不可恢复）；之后从保留目录「重新安装」会按预置来源重建
  登记——该包从此失去 Upload 身份（`edit_display` 动作不再下发），这是安装
  管线的既有来源语义，非本特性引入。

---

## 13. 示例

### 13.1 纯 MCP 包

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

### 13.2 纯技能包

```
greet.zip
├── plugin.json
└── skills/greet/
    └── SKILL.md          # name: greet
```

`plugin.json`：`components.skills: [{ "id": "greet", "dir": "skills/greet" }]`。
（也可省略 `plugin.json`，走裸技能回退。）

### 13.3 组合包（MCP + 技能）

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

## 14. 与其它文档的关系

- `plugin-protocol.md`：协议**设计草案**，含 v2 预留（workflows / cli_connectors /
  commands / hooks / 远程下载等）。本文只落地其 v1 已实施子集。
- `capability-governance.md` / `marketplace-unification.md`：能力包统一模型与治理
  （一个包 = 一张卡 = 一个开关；kind 现算；禁用保证在执行层）的上游依据。
- 本规范由预置市场技能「插件包标准化」`package-author` 落地为可执行指令（见其 `SKILL.md`）。
