# 工具市场统一治理方案（marketplace unification）

> 状态：实施中（Phase 2 app 侧已完成；`docs/marketplace-unification-todo.md` 为已过期的历史交接快照，不再是进度真相源——现行状态以 `docs/plugin-package-spec.md`、`docs/plugin-protocol.md` 与本文为准）。
> 关联文档：`docs/工具市场.md`（现状设计）、`docs/fork-policy.md`（底座改动边界）；
> #287 已合入：`docs/capability-governance.md` 为能力治理单一真相源（scope 已按模式泛化、
> PackDefaultPolicy / declare_all_modes! 编译期哨兵已就位），本文聚焦市场侧的统一改造。

## 0. 背景与决策

工具市场现状是多份权威状态各自演化：`mcp.json`、`marketplace/installed.json`、
`disabled_connectors.json` / `disabled_skills.json`、`bundle/skills/` 扁平目录
（靠 `.installed-from` 标记考古归属）、`connectors/<platform>/bin/` 无版本 CLI 二进制。
#279 完成的能力包统一模型（`features/marketplace/bundle.rs`）目前是**只读视图**，
从上述散落文件反推包信息；写路径仍分散在四条安装管线
（MCP / 预置技能 / 上传 zip / CLI 连接授权）。

**已决策：采用激进路线。** 趁用户基数少、灵活度大，不做投影兼容层的软着陆，
直接让真相源经底座注册缝直供，投影层（mcp.json 写入逻辑、会话技能物化、
凭据占位符）删除而非降级。

## 1. 终态架构

```
┌─ 底座（CodeWhale，三条通用缝，上游优先） ─────────────┐
│  ToolingSource 抽象                                   │
│    ├─ FileToolingSource（默认，读 mcp.json/扫目录）    │
│    └─ RegistryToolingSource（pinvou3 注册，内存直供）  │
│  运行期重配置 tooling_changed(scope)                   │
│  CredentialResolver（调用时惰性解析凭据）              │
└──────────────▲────────────────────────────────────────┘
               │ 注册/事件（无文件、无投影）
┌──────────────┴────────────────────────────────────────┐
│  pinvou3-app · features/marketplace/                   │
│  BundleStore（bundles.json，唯一真相源，可写）          │
│  统一管线：解析→预检→凭据→依赖→登记→供给→热刷           │
│  三态正交：installed(存储) / ready(现算) / enabled(模式)│
│  动作下发：后端出 actions，前端退化为渲染器             │
└────────────────────────────────────────────────────────┘
存储：bundles/<id>/（按包聚合）+ assets/cli/<name>/<version>/（版本化外部资产）+ keyring（凭据）
治理：包 id × SessionMode；三通道=注册排除 + disallowed_tools + execpolicy 硬拦截（含 YOLO）
```

核心原则：**单一真相源，一切派生。** 底座消费的工具面由 BundleStore 直接供给，
不经过任何文件中间表示；任何 PR 不制造双写双读窗口。

## 2. 底座改动（三条通用缝，upstream-first）

进底座的只有通用能力，不掺 Pinvou 语义（遵循 `docs/fork-policy.md`）。
三条缝按可上游化标准设计，同步推上游 PR；上游不接受时 fork 短期沉淀可接受。

| 缝 | 内容 | 顺序与理由 |
|---|---|---|
| 运行期重配置 | `tooling_changed(scope: Session \| Global)` 事件/命令，工具面即时生效 | **先做**：半径最小、价值最高，直接消除"新会话生效"兜底语义 |
| ToolingSource 抽象 | `trait ToolingSource { mcp_servers(); skills(session) -> Vec<SkillSpec> }`；FileSource 为默认实现保持兼容，RegistrySource 由 pinvou3 注册，技能内容内存直供 | 主菜，双轨落地；落地后可删除技能物化模块 |
| CredentialResolver | 凭据引用调用时惰性解析（trait 注册 keyring 实现） | 最后做，独立性强；落地后删除 `${PINVOU3_MCP_SECRET_*}` 占位符体系 |

配套义务：RegistrySource 上线同时交付 `dump_session_tooling` 可观测命令
（替代物化目录的调试价值，"可见即可得"必须与组合机制同时交付）。

## 3. 包模型（终态 schema）

### 3.1 分层

- **存储层 `BundleRecord`**（bundles.json，唯一可写）：id / source
  （`preset` | `upload:<zip名>` | 内置快照 + 内容指纹）/ installed / 内容指纹 /
  assets 引用（name + version + sha256）/ credentials 引用 / 安装时间。
- **查询层 `BundleInfo`**（现算投影，不落盘）：在现有 `bundle.rs:145` 基础上演进：
  - `components: { mcp_servers, skills, cli }`，后续扩展 `commands` / `hooks`；
  - `kind` 由内容现算（沿用 `derive_bundle_kind`，防自报标签提权）；
  - 功能事实：description / version / auth_required / config_fields（V4 已下沉）；
  - `has_executables: bool`（扫描包内技能目录派生，不信自报）；
  - `runtimes` / `pip_dependencies`（软声明，进 readiness 与依赖阶段）；
  - `installed`（存储二态）+ `ready`（查询现算，永不进存储）。

### 3.2 三态正交

| 态 | 性质 | 判定 |
|---|---|---|
| installed | 存储态（二态） | BundleStore 登记 ∧ 资源完整 |
| ready | 派生态（现算） | CLI 按授权存在；凭据型按必填凭据齐；本地免凭据恒 ready；带脚本技能可声明运行时依赖进提示 |
| enabled | 治理态（按模式） | scope map（#287 泛化格式，键=SessionMode） |

异常态收敛为一种：`Degraded`（登记在、资源缺），修复动作统一为按来源重新获取
（预置重释放 / 上传重导入 / CLI 重下载），对所有包类型同构。

### 3.3 动作下发

后端按当前状态下发可用动作集（install / connect / configure / update /
disconnect / uninstall / enable_in(scope)…），每个动作带可用性与原因；
交互流程（飞书流程卡、企微扫码 iframe 等）建模为动作的 flow payload。
前端 `ToolStoreView` 退化为动作渲染器，新增连接器零前端改动。

## 4. 存储布局

```
~/.pinvou3/
├── marketplace/bundles.json        ← 真相源（BundleRecord 集合）
├── marketplace/recycle-bin.json    ← 回收站清单（Upload 包卸载的软删除登记）
├── marketplace/recycle-bin/<id>/   ← 回收站包目录（整包搬移，可恢复/手动彻底删除）
├── bundles/                        ← 每包一个目录，唯一属主
│   └── <id>/
│       ├── mcp/                    ← server 脚本（安装时释放，非启动全量释放）
│       ├── skills/<name>/          ← 包内技能（含脚本；预置/上传/companion 同构）
│       └── archive.zip             ← 上传包原件存档（支持重装）⏳ 终态设计，未实施
├── assets/                         ← 版本化外部资产（包只引用不拥有）
│   ├── cli/<name>/<version>/
│   └── .staging/                   ← 下载/解包暂存（收编 cache/connectors）
├── mcp.json                        ← 过渡期 FileSource 输入；Phase 3 后仅回退，随后删除
└── sessions/<sid>/skills/          ← 物化投影；RegistrySource 落地后删除
```

规则：

1. **一个包 = 一个目录 = 一个属主。** 安装 = staged 解包 + 原子 rename；
   卸载 = 删登记 + 目录按来源处置：Upload 来源（用户唯一副本）整包搬入回收站
   （`marketplace/recycle-bin/<id>/`，含 mcp/ 与 skills/；恢复 = 搬回 + 重走安装
   管线，凭据 secrets 卸载时已删需重填；彻底删除由用户手动触发；条目也可导出为
   符合插件包规范的 zip——plugin.json/mcp/skills 平铺在 zip 根，可经统一导入管线
   重新导入；已安装包（上传/预置/自定义 MCP）同样可导出为标准插件包 zip，
   导出时 `mcp/manifest.json` args 中指向包内的绝对路径还原为相对形式），
   Preset/Builtin 可重释放仍物理删除。`.installed-from` 标记文件取消，
   来源与指纹进 bundles.json。
2. **包内容住包目录，外部依赖住资产库。** CLI 二进制是厂商 URL 下载的版本化外部依赖
   （生命周期由 lock 表驱动，与包登记解耦，卸载时保留/清理可独立决策）；
   技能内脚本是包内容（不可再下载、随包版本变化、属主唯一），不拆。
3. **凭据永不落盘。** keyring 存取经 `bundle::keyring_target` 唯一映射点；
   bundles.json 只存引用（key + target）。
4. **状态与资源分离。** 元数据查询不摸磁盘；磁盘遍历只在完整性校验（指纹比对）时发生。
5. 路径全部走 `platform::paths`，不硬编码。

## 5. 治理模型

### 5.1 三通道对齐（执行点强制）

| 通道 | 层级 | 说明 |
|---|---|---|
| execpolicy deny | **唯一硬保证** | spawn 前硬拒，含 YOLO；规则从包注册表 + scope map 现算；禁用包覆盖带脚本技能（注：底座 DSL 的 command 规则是 word-boundary 前缀、参数位精确匹配，不支持目录前缀，故实现为按脚本文件 × 解释器枚举生成 typed Deny 规则；目录级前缀属底座缝候选） |
| disallowed_tools | 注册面 | MCP 工具名按 scope 排除 |
| 物化排除 / 注册排除 | 体验层 | RegistrySource 落地后由"不供给"实现，物化模块删除 |

纪律：禁用体验可以靠供给层做，禁用保证必须在执行点做；治理态不随权限模式放水；
拦截/授权决策成对落审计日志，模型只见工具结果。

### 5.2 不变量

- **一个包 = 一张卡 = 一个开关。** 包内技能可见性唯一跟随所属包；
  拒绝包内粒度开关（需要单控 = 应该拆包）。
- **kind / has_executables 等内容事实现算，不信自报。** 空包 schema 层拒收（V7）。
- **ready 永不进存储。**
- 开关粒度收敛为包 id × SessionMode；`skill:` 前缀等跨文件借道在迁移中清除。

### 5.3 各类型同构性

| | 包内容（包目录） | 外部依赖（assets 引用） | 授权 |
|---|---|---|---|
| MCP 包 | server 脚本 | pip 依赖（后续收编 `assets/pip/`） | 凭据/OAuth（统一协调器） |
| CLI 包 | companion skills | 厂商 CLI 二进制 | 统一授权协调器（OAuth/扫码/iframe 为 flow 变体） |
| 纯技能包 | SKILL.md 目录（含脚本） | 可选 runtimes/pip 声明 | 凭据型按 credentials |

CLI 无专属生命周期词汇："连接"= connect 动作，"断开"= 删授权；
companion 声明与 MCP 的 `companion_skills` 同机制，`BUILTIN_CLI_BUNDLES`
常量表 manifest 化后退役。检验标准：新增 CLI 连接器 = 加一个 manifest + lock 条目。

## 6. 统一安装管线

四条写路径（MCP / 预置技能 / 上传 zip / CLI 连接）收编为一条管线的阶段组合，
每类包裁剪不适用阶段：

```
install_bundle(id):
  1. 解析   —— registry 查 BundleInfo（含 credentials 声明）
  2. 预检   —— schema 校验（空包拒绝）；validate_on_install 握手（可选）；
              上传 zip 防护（路径穿越/symlink/大小上限）；
              含脚本的上传包 → 显式告知确认
  3. 凭据   —— 按 credentials[].target 统一入 keyring；
              需授权的包插入统一授权子流程（request_id 协调器，五态分类）
  4. 依赖   —— pip_dependencies / AssetManager.ensure(CLI 二进制@lock版本)
  5. 登记   —— 写 BundleStore（原子写；两路资源都成功才算装成，否则回滚到无投影）
  6. 供给   —— 过渡期：重算 mcp.json + 物化；终态：RegistrySource 直接供给
  7. 热刷   —— tooling_changed / refresh_live_sessions / execpolicy ruleset
```

## 7. 插件化路径（预留，不在本方案开工）

- **组合式插件包**：components 向量扩展 `commands` / `hooks` 即为插件包，
  声明/管线/治理/存储全复用；每个新组件类型需要一条对应的禁用通道。
- **进程内代码插件**（dsh 形态）：前提是底座缝上游化 + 信任模型
  （签名、权限在执行点强制、审计）齐备；中间档位优先评估 WASM 沙箱。
- 不做：cordis 式组合层（profile/patch 叠树、HMR、disposer）、插件间依赖系统、
  游离于 BundleStore 之外的第二套扩展体系。

## 8. 分期落地

| 期 | 内容 | 验收 |
|---|---|---|
| **Phase 0** 地基 ✅ | #287 已合入（scope 按模式泛化、capability-governance.md、编译期穷尽哨兵） | 已上游合并 |
| **Phase 1** 底座缝 | tooling_changed → ToolingSource 双轨 → CredentialResolver；上游 PR 同步推（探查已完成，落点见 todo 文档 B 节） | FileSource 行为不变；RegistrySource 可注册；dump_session_tooling 交付 |
| **Phase 2** app 反转 ✅ | BundleStore 可写化 + 首启迁移（刀1-3）；存储布局迁移（刀4/10/11）；execpolicy 路径 deny（刀6）；scope 收敛包 id（刀12 / a02d58b7，见 todo A 节，已完成） | 安装/卸载/更新镜像全走 BundleStore；bundles.json 为安装态唯一真相源 |
| **Phase 3** 切换删除（前端部分已提前） | 前端切 bundle_readiness + 动作下发 ✅（刀5/7/8/9，CLI 元数据下沉已同批完成）；待做：app 切 RegistrySource、删除物化模块、mcp.json 写入、占位符、CONNECTOR_CLAIMED_SKILLS、逐连接器 status 命令（前端调用已删，后端命令待删）、tsSkillsData、CLI 常量表 | 删除清单逐项核销；FileSource 回退期（一个版本周期）后删除 |
| **Phase 4** 收尾 | V5 条件认领退出条件执行；占位卡改注册表 upcoming 条目；capability-governance.md 登记全部已知限制 | 文档与代码一致 |

每期内"新路径 + 旧路径删除"原子完成，不留双写双读中间态（#279"前端已切、
元数据没搬"硬约束扩展为全程通则）。

## 9. 迁移（首启一次性导入）

1. 扫 `bundle/skills/*` 的 `.installed-from` 反推归属（`pinvou3-marketplace:<id>` →
   对应包；`upload:<zip>` → 独立上传包；lark-*/wecomcli-* 等 → 对应 CLI 包），
   移目录至 `bundles/<id>/skills/`，登记内容指纹；
2. `mcp.json` + `installed.json` 反推已装 MCP 包，登记并补齐资源完整性校验；
3. 存量 CLI 二进制对照 lock 表验 SHA-256：匹配则移入 `assets/cli/<name>/<version>/`
   并登记版本，不匹配则登记 `Degraded`（待修复，提供重新下载动作）；
4. 全程在 FILE_LOCK 内完成（"读到即迁移"必须持锁，#287 竞态教训前置）；
   迁移幂等；失败回滚到 FileSource，保留"重新安装即可自愈"逃生门；
5. 迁移校验通过后旧布局直接删除（激进红利：用户基数少，不保留双轨）。

## 10. 贯穿纪律

1. **无投影漂移**：真相源只有一个；契约测试钉住接口行为（非文件格式）。
2. **决策点强制**：治理硬保证在 execpolicy / disallowed_tools 执行层；
   prompt/供给层只是体验。
3. **编译期强制优先**：新增模式、组件类型、CLI 清单，能宏哨兵/E0004 的不靠
   测试遍历（#287 自我指涉测试教训：`ALL` 遍历 `ALL` 漏挂照样绿）。
4. **可观测性与组合同时交付**：`dump_session_tooling` 随 RegistrySource 上线。
5. **审计成对**：授权/拦截/迁移决策落事件日志。
6. **fork 边界**：底座只进三条通用缝；每次 gitlink 更新跑 `fork-guard.sh --fast`
   + 契约测试；fork-distinct 行为同步登记 `docs/fork-modifications.md`。
7. **Validation baseline**: high-risk ready PRs and high-risk merge groups run
   `cargo test --lib -- --test-threads=1` on the combined tree; main push keeps
   cumulative compile verification (`--no-run`) and cache warming. Bundle tests share
   `platform::paths::tests::ENV_LOCK`; `cargo fmt --check` and
   `architecture-guard.py` must pass.

## 11. 风险与对策

| 风险 | 对策 |
|---|---|
| 上游不接受 ToolingSource | fork 短期沉淀可接受（用户少 = 同步冲突面小）；缝按可上游化标准设计，不掺 Pinvou 语义 |
| Phase 3 cutover blast radius | Compare old/new data sources field by field; require full serial PR regression plus the combined-tree Merge Queue gate |
| 存量用户迁移失败 | 迁移幂等 + 失败回滚 FileSource + 重装自愈逃生门 |
| 技能不落盘损失可调试性 | dump_session_tooling + 审计日志补上，列入 Phase 1 验收 |
| 过渡形态沉淀为永久复杂度 | V5 条件认领等过渡设计登记退出条件（本文 §8 Phase 4），capability-governance.md 跟踪 |

## 12. 设计溯源

本方案的抽象纪律参照 deepseek-harness（`.luzeyang/deepseek-harness`）的插件体系：
一切皆插件无特权核心（→ 底座缝）、声明式组合显式优于隐式（→ 注册表无扫描）、
安装与激活分离（→ installed/ready 分离）、效果可逆 fail loud（→ 原子管线 + 空包拒收）、
决策点强制（→ execpolicy 硬拦截）、单一真相源派生一切（→ BundleStore + 删除投影层）、
Known Limitations 明文化（→ 本文 §11 + capability-governance.md 登记）。
不引入：cordis 组合层、HMR/disposer、进程内插件运行时（现阶段无对应需求）。
