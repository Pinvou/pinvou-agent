# 插件协议（Plugin Protocol）设计

> 状态：**v1 子集已实施**（MCP / 技能 / 组合包，落地版见 `plugin-package-spec.md`），
> 其余仍为设计草案。本文定义「应用商店」用户上传 zip 插件包的
> 包格式、自动识别、落盘与读取协议。落地基线为能力包统一模型
> （`docs/marketplace-unification.md` §3/§4/§6，Phase 2 全部 12 刀已合入）。
> 本文是 `marketplace-unification.md` §7「插件化路径」的首个落地切片：把
> **组合式插件包**（components 向量扩展）从「预留」推进到「协议定稿」，
> 但**不含**进程内代码插件（dsh 形态）——那条路径仍需底座缝 + 信任模型
> （签名、执行点权限、审计）齐备，见 §12 开放决策。

---

## 1. 目标与范围

**目标**：用户上传一个 zip，无论其内容属于哪种能力类型（MCP server、Skill、
工作流、或它们的组合），应用都自动识别类型、安全校验、落盘到按包聚合的目录，
并在商店与运行时按同一套「包」模型读取/开关/卸载。

**范围**（v1 组件类型）：

| 组件类型 | v1 是否识别 | 说明 |
|---|---|---|
| `mcp_servers` | ✅ | 本地 stdio / 远程 OAuth MCP，复用现有 `ToolManifest` |
| `spanner` | ⛔ 已移除 | 扳手插件独立组件模型已从 v1 移除，原声明式 one-shot 设计保留为历史记录（§15）；可执行能力的演进方向是 skill 包 `tools[]`/`runtime` + skill-run wrapper，**该方向为 RFC 草案，执行通路未实施** |
| `skills` | ✅ | SKILL.md 目录（挂载给 LLM 的 markdown 指令），复用现有技能落盘与物化 |
| `workflows` | ❌ v2 | harness loop 多智能体编排；识别/落盘可行，但治理通道需 harness 侧 scenario 过滤缝（§11/§12-B），v1 不做 |
| `cli_connectors` | ⏳ 未实现（草案） | zip 只放「下载声明」（URL + SHA-256 + 版本 + 配套技能），安装时下载二进制到 `assets/cli/`（lock 表）；不内嵌原生二进制，详见 §14.3 |
| `commands` / `hooks` | ⛔ 预留 | 见 §12，需新增执行点禁用通道与信任模型 |

**不变量**（沿用 `capability-governance.md` §5.2 / `marketplace-unification.md` §5.2）：
一个插件包 = 一张卡 = 一个开关；`kind` 与 `has_executables` 等事实**现算**，
不信自报标签；空包 schema 层拒收；凭据永不落盘。

---

## 2. 术语与定位

- **能力包（Bundle）**：既有统一模型 `Bundle = { id, name, mcp_servers, skills,
  cli, … }`（`features/marketplace/bundle.rs`）。
- **插件包（Plugin package）**：能力包的**用户上传 zip 形态**，不是第二套扩展
  体系。两者共用 `BundleRecord`（存储）、`BundleInfo`（查询）、`BundleKind`
  （类型）、动作与治理。
- **插件清单 `plugin.json`**：插件包根目录的权威声明文件，是 zip 内跨组件
  「一个包」的粘合层。它**声明**组件、凭据、依赖，但类型/脚本事实由解包后的
  内容**校验**后现算（§5）。

---

## 3. 插件包结构（zip 布局）

```
my-plugin.zip
├── plugin.json                     ← 插件清单（权威；缺省则走结构回退，§5.2）
├── mcp/                            ← MCP server（扁平单目录；v1 一包一 server）
│   ├── manifest.json
│   └── server.py
├── skills/<skill-name>/            ← SKILL.md 目录（可多个）
│   ├── SKILL.md
│   └── references/…
├── workflows/<workflow-id>/        ← 工作流目录（可多个）
│   ├── workflow.json
│   └── scripts/…
└── (其它静态资源随组件子树同打包)
```

约定：

1. 组件按**目录约定**分域（`mcp/`、`skills/`、`workflows/`），与落盘后的
   `bundles/<id>/` 布局同构，导入时映射直搬，无需二次解释。
2. `plugin.json` 里的组件路径是**相对 zip 根**的目录引用，缺失即失败
   （声明与实际不符 fail loud，§5.1）。
3. 兼容「裸包」：没有 `plugin.json` 的 zip 走结构回退（§5.2），等价于现有
   上传技能包的行为，零破坏。

---

## 4. 插件清单 `plugin.json`（schema v1）

`manifest_version` 是协议版本（本文 = 1）；`version` 是插件自身的语义版本。

```jsonc
{
  "manifest_version": 1,
  "id": "weather-insight",              // 包 id，落盘目录名；[a-z0-9-_]{1,64}
  "name": "天气洞察",                    // 展示名
  "version": "1.2.0",                   // 插件语义版本
  "description": "…",                   // 功能事实
  "author": "…",
  "license": "MIT",
  "homepage": "https://…",

  // 组件声明（目录引用，导入时校验存在；v1 已实施：mcp_servers 与 skills，
  // 其中 MCP 组件 dir 必须精确为 "mcp"——一包一 server，见 §5.1 与
  // plugin-package-spec.md §2；多 server 为本协议 v2 草案方向）
  "components": {
    "mcp_servers": [
      { "id": "weather", "dir": "mcp" }
    ],
    "skills": [
      { "id": "weather-interpret", "dir": "skills/weather-interpret" }
    ],
    "workflows": [
      { "id": "weather-report", "dir": "workflows/weather-report" }
    ]
  },

  // ⏳ 凭据声明（未实施：v1 解析落入 extra 不消费；凭据仍声明在
  // mcp/manifest.json 的 ToolManifest 通道，见 plugin-package-spec.md §6）
  "credentials": [
    { "key": "AMAP_KEY", "target": "env", "required": true },
    { "key": "QCC_TOKEN", "target": "bearer", "required": false }
  ],

  // ⏳ 配置弹窗字段（未实施，同上）
  "config_fields": [
    { "key": "AMAP_KEY", "required": true, "target": "env", "secret": true }
  ],

  // ⏳ 依赖软声明（未实施：仅存 extra，无 readiness/依赖阶段消费）
  "dependencies": {
    "runtimes": ["python3"],
    "pip": ["requests"]
  }
}
```

字段语义与既有结构对齐：

| 字段 | 去向 | 备注 |
|---|---|---|
| `id` | `BundleRecord.id` + 目录名 | 复用 `is_safe_skill_name` 泛化为 `is_safe_component_id` |
| `components.mcp_servers[].id` | `BundleInfo.mcp_servers` | 对应 `ToolManifest.id`；其 `manifest.json` 仍是该 server 的启动真相源 |
| `components.skills[].id` | `BundleInfo.skills` | = SKILL.md frontmatter `name`，导入时校验一致 |
| `components.workflows[].id` | `BundleInfo.workflows`（新增） | = `workflow.json` 的 `id`，导入时校验一致；workflows 通道模块待建（v2，见 §8） |
| `credentials` / `config_fields` | `BundleInfo.credentials` / `config_fields` | ⏳ 设计去向；v1 落 `extra` 不消费，实际凭据收敛仍走 `bundle::tool_credentials` / `tool_config_fields`（ToolManifest 通道） |
| `dependencies` | `BundleRecord.extra`（v1 只记录） | ⏳ 后续收编 `assets/pip/` 时进依赖阶段 |

前向兼容纪律（与 `store.rs` 同款）：`plugin.json` 解析**不用**
`deny_unknown_fields`，未知字段经 flatten 保留；`manifest_version` 高于当前支持
版本时**拒装并提示升级应用**，不静默降级。

---

## 5. 类型自动识别

### 5.1 原则

- **manifest 优先**：根有 `plugin.json` → 按声明解析，逐组件校验目录存在 →
  组件齐了才定类型。
- **内容现算，不信自报**：`kind` 由「校验通过后的组件向量」推导，`plugin.json`
  里没有 `kind` 字段——声明与实际不符（声明 skill 却无 SKILL.md）即失败，
  杜绝「自报标签提权」（`marketplace-unification.md` §5.2 / `capability-governance.md` §3.1）。
- **结构回退**：无 `plugin.json` 的裸包按目录结构识别（§5.2），兼容既有上传技能
  zip 与未来「直接拖一个 MCP 脚本目录」的用例。

### 5.2 识别算法

```
detect_plugin(zip):
  entries = 安全枚举（§9 防护已套）
  if 根有 plugin.json:
      parse manifest; manifest_version 校验
      components = 逐项校验目录存在 + 关键文件存在（SKILL.md / workflow.json / mcp manifest）
      kind = derive_bundle_kind(components)      # 见 §6
      return Typed(manifest, components, kind)
  else:                                       # 结构回退
      skill_roots = 找 SKILL.md（rank 排序，复用 skill_marketplace 现有逻辑）
      mcp_roots   = 找 mcp 型 manifest.json（含 command/servers 字段）
      workflow_roots = 找 workflow.json（含 id + scenarios 或 ui）
      if 全空: 拒收（空包，V7 纪律）
      kind = derive_bundle_kind(components)
      return Bare(components, kind)           # 生成派生 manifest 落盘
```

结构回退的 MCP 识别规则（与实现对齐）：

- 精确路径 `mcp/manifest.json` 且能解析、`id` 非空 → 视为 MCP server（不要求
  `command`/`servers` 字段）。
- zip 内**任意其他位置**的 `manifest.json` 能解析出 `ToolManifest`、`id` 非空且
  具备 `command` 或 `servers` 字段 → 视为 MCP server，规范化入 `mcp/`。

> 现状注记（六轮评审补）：当前实现（`plugin_import.rs::detect_components`）
> 只识别 `mcp_servers` / `skills` 两类组件（含裸技能/裸 MCP 回退），伪代码中的
> `workflow_roots` 与 §6 的 `Workflow` 变体为 v2 草案（§1 表），`derive_bundle_kind`
> 现行签名为 `(mcp_servers, skills, cli)`，不含 workflows。

### 5.3 识别结果（`PluginImportReport`）

现状（六轮评审更正，与代码一致）：统一导入管线的返回类型是
`PluginImportReport`（`plugin_import.rs`），仅三个字段：

```rust
pub struct PluginImportReport {
    pub id: String,
    pub kind: BundleKind,
    /// 落盘后的图标相对路径（`icon.svg`/`icon.png`）。
    pub icon: String,
}
```

- 识别阶段的中间结果是**私有** `ComponentDetection { id, mcp_servers, skills,
  bare_skill, bare_mcp }`（`plugin_import.rs`，不跨模块暴露），组件向量当前只有
  `mcp_servers` / `skills` 两类，无 `workflows`。
- `PluginImportReport` 不含 `components` / `has_executables` / `manifest`：
  组件清单随规范化 `plugin.json` 落盘在 `bundles/<id>/`（裸包由导入层合成
  派生清单，带声明的包按解析后规范化字节写回），`BundleRecord` 登记
  `credential_keys` / `content_fingerprint` 等镜像字段（§7）；`has_executables`
  显式确认流程为 §9.5 目标形态（⏳ 未实现），规范化 manifest 副本不回传调用方。
- 早期草案的五字段 `DetectedPlugin` 类型从未落地，已按现状删除；若未来需要
  向命令层回传组件向量，再按「目标形态」单独立项并回改本节。

---

## 6. 组件模型与 kind 推导扩展

`BundleKind` 增补一个变体，组件向量增补 `workflows`：

```rust
pub enum BundleKind {
    Cli,        // cli 非空
    Bundle,     // 跨组件组合（mcp+skill / mcp+workflow / skill+workflow …）
    Mcp,        // 仅 mcp_servers
    Skill,      // 仅 skills
    Workflow,   // 仅 workflows   ← 新增
}
```

`derive_bundle_kind` 优先级（在 `bundle.rs` 现有纯函数上扩展，保持 Cli 恒赢）：

```
cli 非空                                    → Cli
mcp 非空 && (skills 或 workflows 非空)        → Bundle
mcp 非空                                    → Mcp
skills 非空 && workflows 非空                → Bundle
skills 非空                                 → Skill
workflows 非空                              → Workflow
全空                                        → Err(InvalidBundle)
```

`BundleInfo` 增补 `workflows: Vec<String>`；`BundleRegistry::list_bundles` 新增
「上传插件源」（与现有上传技能源并列，读取 `BundleRecord.source=Upload` 且
包目录带 `plugin.json` 的记录）。`actions_for` 对 `Workflow` 走 `Mcp/Skill`
同支的安装/卸载动作（工作流无凭据则免凭据 `install`，有则 `configure`）。

---

## 7. 落盘布局（`bundles/<id>/` 扩展）

沿用 `marketplace-unification.md` §4「一个包 = 一个目录 = 一个属主」，增补：

```
~/.pinvou3/bundles/<id>/
├── plugin.json                    ← 规范化插件清单（自描述；裸包导入时由识别层生成）
├── mcp/                           ← MCP server 脚本 + manifest.json（扁平单目录，同现状）
├── skills/<skill-name>/           ← SKILL.md 目录（同现状）
├── workflows/<workflow-id>/       ← workflow.json + scripts/  ← 新增
└── archive.zip                    ← 上传原件存档（repair/update 用，§4 已预留）⏳ 未实施
```

落盘规则：

1. 解包到 `bundles/<id>/.stage/` → 校验（§9）→ 原子 rename 到 `bundles/<id>/`
   （与技能/MCP 现有 staged + rename 同范式）。
2. `archive.zip` 存**原始字节**（非二次压缩）——⏳ 未实施：当前导入管线不写
   archive.zip，repair/update 场景需用户重新提供原件；`content_fingerprint` 覆盖
   解包后的内容目录（`skill_marketplace::dir_fingerprint` 同口径，跳过隐藏/
   标记文件）。
3. 登记 `BundleRecord { id, source: Upload("<zip名>"), installed: true,
   content_fingerprint, credential_keys, assets: [], … }`，保持 store 前向兼容。
   （现状：`credential_keys` 由 `tool_credentials` 从落盘 manifest 收敛、
   `content_fingerprint` 走 `dir_fingerprint` 同口径；「插件语义版本与组件清单
   进 `extra`」未实施——组件清单由 `bundles/<id>/plugin.json` 自描述承担。）
4. 纯 Skill 插件落盘后与现有上传技能**目录完全同构**（`bundles/<id>/skills/<name>/`），
   因此既有物化/开关/卸载路径零改动即可消费。

---

## 8. 读取/供给（运行时如何「读」到）

「落盘读取」= 查询层 + 各组件供给通道，全部现算、无新增中间投影：

| 组件 | 供给通道（现状复用） | 说明 |
|---|---|---|
| `mcp_servers` | `MarketplaceManager::available_tools` 读 `bundles/<id>/mcp/manifest.json` → 写 `mcp.json` 供底座 FileSource | 上传 MCP 首次进入 `available_tools` 的新布局扫描路径（旧 `bundle/mcp-servers/` 布局退役已合入 main） |
| `skills` | `SkillMarketplaceManager::find_skill_dir` + 会话组合目录物化（`skill_materialization`） | 纯 Skill 插件与现状上传技能同构 |
| `workflows` | `workflow_registry::discover` 扫描 `bundle/workflow/` | ⏳ 模块待建（v2）：`discover` 增补 `bundles/<id>/workflows/` 来源，或导入时把 workflow 目录同步到 `bundle/workflow/`（见 §12 决策 D） |

查询层 `BundleRegistry::list_bundles` 汇总上传插件源，商店卡片由后端 `BundleInfo`
合成（前端继续退化为动作渲染器）。

---

## 9. 安全边界（导入与读取）

沿用并泛化现有 `skill_marketplace::import_package` 的防护，成文为统一预检：

1. **路径穿越**：每个 zip entry 用 `enclosed_name()`，`None` 即拒（`..`/绝对路径）；
   写出前 `target.starts_with(staged)` 二次断言。
2. **symlink / hardlink**：`unix_mode` 高 4 位为 `0o120000` 即拒。
3. **体积与数量上限**：单包解压累计 ≤ `MAX_PLUGIN_SIZE_BYTES`（200 MiB，
   可随组件类型放宽为可配置常量）；entry 数量上限防 zip 炸弹；压缩前大小
   （`entry.size()` 累加）与压缩率双重卡。
4. **名字安全**：`is_safe_skill_name` 泛化为 `is_safe_component_id`
   （`[A-Za-z0-9-_]{1,64}`，禁 `.`/`..`/路径分隔符），组件 id、server id、
   skill name、workflow id 一律套用；与已下线内置名冲突即拒。
5. **脚本事实现算 `has_executables`**（⏳ 未实现，草案）：解包后扫描技能/workflow/MCP 子树中的
   可执行脚本（`.py`/`.sh`/`.ps1`/`.js`/`.cjs`/`.mjs` 等按解释器表判定）。
   为 true 时，导入/安装前**显式告知用户**「该包含可执行脚本，安装即授权其
   以本机身份运行」（`marketplace-unification.md` §6 预检语义落地）。
6. **凭据不落盘**：`credentials`/`config_fields` 只写 keyring 引用，`mcp.json`
   只落 `${PINVOU3_MCP_SECRET_*}` 占位符（现制）。
7. **读取侧**：目录只由包 id 定位，包 id 经 `is_safe_component_id` 校验后再
   拼路径，杜绝「上传包 id 被用于路径穿越」。

---

## 10. 统一导入管线

把现有「上传 zip 技能」收编为「上传插件包」的阶段组合（`marketplace-unification.md`
§6 的 upload 分支），单入口 `import_plugin_package(zip, display_name)`：

```
1. 解析     —— 安全枚举 + 识别（§5）→ 内部 `ComponentDetection`（§5.3）；manifest 校验
2. 预检     —— schema（空包拒收）/ 穿越 / symlink / 体积 / 名字 / has_executables 确认
3. 凭据     —— 按 credentials[].target 入 keyring（v1 上传后按需在 install/configure 阶段收集）
4. 依赖     —— 记录 dependencies 进 extra；pip 安装延后到 install（同现有 MCP 管线）
5. 登记     —— staged 解包 → 原子 rename → 写 BundleRecord（archive.zip 存档 ⏳ 未实施）
6. 供给     —— 重算 mcp.json（含 MCP）/ 会话技能物化（含 skills）/ workflow 发现（含 workflows）
7. 热刷     —— refresh_live_sessions_skills + execpolicy ruleset（含脚本包的 deny 规则）
```

现状（四轮评审更正，与代码一致）：统一管线由**新命令** `import_plugin_package_cmd` /
`import_plugin_package_bytes_cmd`（对话框 / 拖放 base64）暴露，内部走
`plugin_import::import_plugin_package`；旧命令 `import_skill_package` /
`import_skill_package_bytes` **入口名与旧管线均保持不变**（仍走
`SkillMarketplaceManager::import_package`），四条命令返回类型都是
`Result<bool, String>`（true=已导入，false=用户取消），未演进为
`PluginImportReport`。`PluginImportReport` 目前仅是管线内部与
`import_skill_md_content`（.md 包装导入）的返回类型，不暴露到命令层。

---

## 11. 治理通道（每个组件一条禁用通道）

沿用「包 id × SessionMode」单一禁用集（`disabled_bundles.json`），禁用保证在执行层：

| 组件 | 禁用通道 | 状态 |
|---|---|---|
| `mcp_servers` | `disallowed_tools` 按 `mcp_{server}_*` 排除 | ✅ 现状 |
| `skills` | 物化排除 + execpolicy deny（带脚本时按脚本×解释器生成 typed Deny） | ✅ 现状 |
| `workflows` | **新增**：`workflow_registry::discover` 按禁用包过滤 scenario 解析 | ⛔ 模块待建（v2），见 §12 D |
| `commands`/`hooks` | execpolicy（预留） | ⛔ §12 |

纪律不变：禁用体验可走供给层，禁用保证必须在执行层（`capability-governance.md` §5.1）。

---

## 12. 开放决策（需产品/架构拍板后再实施）

- **A. 清单文件名**：建议 `plugin.json`（与底座 `skills::install` 已识别其为
  「插件非技能」的语义一致，且不与 MCP `manifest.json` / `SKILL.md` 冲突）。
  备选 `pinvou.json`。
- **B. 工作流是否入 v1**：`workflows` 的治理通道（§11）尚需 harness 侧加
  scenario 过滤缝；若不入 v1，则 v1 仅 `mcp_servers` + `skills`，`workflows`
  与 `commands`/`hooks` 一并列为 v2。
- **C. 上传即安装 vs 上传后安装**：现技能上传是「上传即安装」。插件包含脚本/
  凭据时，是否仍上传即装，还是落为「已上传未安装」待用户显式 `install`？
  影响 `BundleRecord.installed` 的写入时机与 `has_executables` 确认流。
- **D. workflow 落盘来源**：`discover` 直读 `bundles/<id>/workflows/`（推荐，
  单一真相源）vs 导入时复制到 `bundle/workflow/`（双写，违背无投影漂移）。
- **E. 体积上限策略**：5 MiB 是技能包的防御值；MCP/工作流包是否需要更大的
  独立上限或分级上限。
- **F. CLI 连接器如何上传**：**v1 只走「声明式」**（§14.3）——zip 内**禁止**塞
  原生二进制（等于无沙箱任意代码执行，需签名/权限/审计信任模型，留 §7 进程内
  插件档）。声明式 = zip 只放下载 URL + SHA-256 + 版本 + 配套技能，安装时下载到
  `assets/cli/<bin>/<version>/` 并验 SHA-256，信任面是「下载并执行第三方二进制」，
  故安装前**显式告知**用户 + SHA-256 强制。治理通道复用现有 CLI 的 execpolicy
  deny（按 `bin` 名 spawn 前硬拒），见 §11。

---

## 13. 与既有实现的衔接

- `store.rs` 的 `BundleSource::Upload(<zip名>)` 直接承载插件包来源，**无需新增
  枚举变体**；插件语义版本/组件清单进 `BundleRecord.extra`。
- `bundle.rs` 的 `derive_bundle_kind` / `BundleKind` / `BundleInfo` 按 §6 扩展，
  是唯二的结构性改动点（外加 `BundleRegistry::list_bundles` 增补上传插件源）。
- `skill_marketplace.rs` 的 zip 防护（`enclosed_name` / symlink / 大小 / rank）
  抽为共享 `plugin_import` 模块复用，技能导入收编为「只含 skills 组件的插件导入」
  （⏳ 待办：现状仅 `read_zip_entry_bounded` 已共享，旧技能导入命令仍走
  `SkillMarketplaceManager::import_package` 旧管线，见 §10 现状注记）。
- 旧 `bundle/mcp-servers/` 布局退役与 `available_tools` 只读新布局已合入 main
  （`marketplace/mod.rs`），是 MCP 组件进入统一读取路径的前置。

---

## 14. 远程下载通道（Remote Fetch）

**目标**：插件包的获取不限于「本地上传 zip」，还支持「远程 URL 下载」，并允许
包内组件声明为远程资源（安装时再拉取）。远程下载**不是第二条导入管线**，只是
统一导入管线（§10）前面的一个「获取」阶段。

### 14.1 三个层级

| 层级 | 触发 | 通道 | v1 |
|---|---|---|---|
| 包级下载 | 商店输入 URL / 远程分发 | `import_plugin_package_from_url` → 下载 → 复用 §10 管线 | ⏳ 未实现（草案） |
| 组件级远程资源 | `plugin.json` 声明 `source:"remote"` | 安装时 AssetManager 下载并验 SHA-256 | ⏳（仅内置 CLI 走该管线；plugin.json 声明式触发为草案）|
| 依赖级下载 | `dependencies.pip` / `runtimes` | 现有 pip / runtime 管线（§4，软声明） | ⏳（pip 管线现仅消费 `mcp/manifest.json` 的 `pip_dependencies`；`plugin.json` 的 `dependencies` 键未消费，见 §10） |

### 14.2 包级下载（商店 URL 导入）

拟议命令 `import_plugin_package_from_url(url, display_name, sha256?)`（未实现，草案）：流程：

```
1. 获取   —— 下载到 assets/.staging/<hash>.zip（HTTPS only）
2. 校验   —— 若提供 sha256 则强制匹配；未提供则告警后仍导入（记 degraded 可选）
3. 导入   —— 复用 §10 统一导入管线（import_plugin_package(bytes) 同一条）
```

- **来源登记**：`BundleRecord.source` 沿用 `Upload("<zip名>")`，`extra` 里加
  `{"origin_url":…, "sha256":…}`；不新增枚举变体（与 §13 的 Upload 复用同纪律）。
- **安全**：下载大小上限同 §9.3（压缩前大小 + 解压后大小双重卡）；超时/重试；
  解包后仍走 §9 全部防护（穿越/symlink/名字/`has_executables` 确认）。
- **信任模型**：sha256 与 URL 同源时只能防「传输篡改」，不能防「URL 本身是恶意
  内容」——因此远程导入的解包/脚本确认流与本地上传**完全一致**，不因来源是 URL
  而放松（`has_executables` 显式告知照常）。

### 14.3 组件级远程资源（plugin.json 声明）——声明式 CLI 连接器

`plugin.json` 的组件可声明为远程资源，包目录只放「下载声明」，不塞二进制：

```jsonc
"components": {
  "cli_connectors": [
    {
      "id": "lark",                     // 连接器/包内组件 id（is_safe_component_id）
      "bin": "lark-cli",                // lock 表 bin 名（execpolicy deny 与 spawn 解析用它）
      "version": "1.0.65",
      "url": "https://…/lark-cli-1.0.65-win-x64.zip",
      "sha256": "…",                    // 强制；不匹配即安装失败
      "skills_dir": "skills/lark"       // 可选：配套官方技能目录（解包进 bundles/<id>/skills/）
    }
  ]
}
```

安装流程（复用内置 CLI 的 `connector_lock` 机制，来源从「内置常量表」换成
「上传声明」）：

```
1. 下载   —— AssetManager.ensure(bin@version) 下载到 assets/cli/<bin>/<version>/
2. 校验   —— SHA-256 强制匹配；失败 → Degraded（重新下载动作），不落半套
3. 技能   —— skills_dir 解包到 bundles/<id>/skills/（复用 §7 技能落盘）
4. 登记   —— BundleRecord { source: Upload, installed: true, assets:[<lock引用>], … }
5. 治理   —— execpolicy deny 按 bin 名 spawn 前硬拒（复用现有 CLI 治理，§11）
```

- **信任面**：声明式 CLI 仍是「下载并执行第三方二进制」，故安装前**显式告知**
  用户（含第三方可执行文件 + 来源 URL），且 SHA-256 强制（不允许无校验和导入）。
- **平台差异**：`url` 需按平台/架构分列（`url` 或 `platforms:{ "windows-x64":…, "darwin-arm64":… }`），
  复用 `connector_lock` 的平台目录解析（`connector_platform_dir`）。
- **不内嵌二进制**：zip 内出现 ELF/PE/Mach-O 等原生可执行文件 → 拒收（§9 扩展
  检测），杜绝「绕过下载走内嵌」的任意代码执行面。
- 其他大体积远程资源（如 MCP 的模型权重、工作流数据）同样走
  `source:"remote"` + 版本钉住，落 `assets/<kind>/<id>/<version>/`，包目录只留声明。

### 14.4 与现有资产库的关系

- 包内容（脚本/SKILL.md/workflow）→ 住 `bundles/<id>/`（包拥有，随包版本）。
- 外部依赖（CLI 二进制/大体积远程资源）→ 住 `assets/`（版本化、lock 表驱动、
  包只引用不拥有）——沿用 `marketplace-unification.md` §3/§4 的既有边界，远程
  下载只是给「外部依赖」补上一条**由插件包声明触发的获取通道**，不改变属主划分。

---

## 15. ~~扳手插件 `spanner`（one-shot）~~（已移除）

> 2026-08：spanner 独立组件模型已移除。可执行能力的演进方向是 skill-with-runtime
> 协议（SKILL.md frontmatter `tools[]` + `runtime` 段声明 + skill-run wrapper）——
> **该方向为 RFC 草案，执行通路未实施**（无 install 后置 hook、无 priority_paths、
> 无 skill-run wrapper 二进制）。本节历史内容仅存档。

**定位**：介于「脚本 skill（太自由）」和「MCP（太重）」之间。声明式 schema +
无状态单次进程 + 自带运行时，语言不限。作者只写「读 stdin JSON → 写 stdout
JSON」，不用懂 MCP 协议；结构化、可校验、可治理。

### 15.1 包形态

```
my-spanner.zip
├── plugin.json            ← 必需（spanner 需要 schema/runtime/entry 声明）
├── spanner/                  ← 工具逻辑（entry + 辅助文件）
│   ├── main.py            ← 入口：stdin JSON → stdout JSON
│   └── …（辅助文件）
├── runtime/               ← 可选：自带运行时（有则进资源池去重）
└── icon.svg|png           ← 可选图标（缺省落盘默认图标）
```

### 15.2 plugin.json（spanner 声明）

```jsonc
{
  "manifest_version": 1,
  "id": "weather",
  "name": "天气查询",
  "version": "1.0.0",
  "description": "查城市天气",
  "icon": "icon.svg",                       // 可选，相对 zip 根；缺省用默认图标
  "spanner": {
    "entry": "spanner/main.py",
    "runtime": { "kind": "python", "dir": "runtime" },  // 可选；缺省用内置 python
    "input_schema":  { "type":"object", "properties": {"city":{"type":"string"}}, "required":["city"] },
    "output_schema": { "type":"object" },
    "timeout_secs": 30,
    "background": false
  }
}
```

### 15.3 调用约定（作者视角）

```
引擎 spawn <runtime> main.py
  → 参数 JSON 写 stdin
  → main.py 读 stdin → 干活 → 结果 JSON 写 stdout
  → 引擎读 stdout → 结构化工具结果
```

作者只写：

```python
import json, sys
args = json.load(sys.stdin)
result = do_work(args)
json.dump(result, sys.stdout)
```

### 15.4 供给（运行时如何变工具）

spanner 包**不自带 MCP 协议**，由内置 `spanner_runner`（通用 MCP 适配器）包装：

- 安装时生成 mcp.json 条目：`command = <运行时>`，`args = [spanner_runner.py, <spanner 清单路径>]`。
- `spanner_runner` 读 spanner 清单 → 暴露**一个** MCP 工具（name/schema 来自声明）→
  调用时 spawn `entry`，stdin JSON → stdout JSON → 回传结果。
- 复用整个 MCP 管线（FileSource 拉起 / 工具发现 / 调用 / disallowed_tools 治理），
  **零底座缝**。

### 15.5 kind 推导扩展

`BundleKind` 增补 `Spanner`；`derive_bundle_kind` 增补 `spanners` 维度，优先级：

```
cli 非空                          → Cli
mcp 非空 && (skills 或 spanners 非空) → Bundle
skills 非空 && spanners 非空          → Bundle
mcp 非空                          → Mcp
skills 非空                       → Skill
spanners 非空                        → Spanner      ← 新增
全空                              → Err
```

### 15.6 图标（可选 + 缺省默认）

- **可选**：zip 根放 `icon.svg`/`icon.png`，`plugin.json.icon` 引用；导入时落到
  `bundles/<id>/icon.<ext>`（**图标与工具同目录**）。
- **缺省**：无图标 → 落盘时写内置默认图标 `bundles/<id>/icon.svg`。
- **查询**：`BundleInfo` 增补 `icon: Option<String>`（相对包目录的图标路径）；
  前端「已装工具」一律读 `bundles/<id>/icon.*`，不再依赖前端硬编码图标表。

### 15.7 与 MCP / skill 的关系

| | spanner | MCP | skill |
|---|---|---|---|
| 结构化 | ✅ 静态 schema | ✅ schema | ❌ 自由 |
| 语言 | ✅ 不限（自带运行时） | 基本锁 python | 靠系统环境 |
| 实现成本 | 低（stdin→stdout） | 高（协议/连接） | 低 |
| 常驻/流式 | ❌ 无 | ✅ 有 | ❌ |
| 治理 | disallowed_tools | disallowed_tools | execpolicy + 物化 |

**互补不替代**：spanner 管「低频、快速、无状态、单次」；MCP 管「高频、有状态、
流式、多工具常驻」。
