# 08a · 静态资产层 Static Assets

> 架构层文档 · 通用 · 资源模块三类之一（总览见 `08-resources`）
> 静态资产 = 工作流自带的固定只读资源（base.css / design_tokens / L01-L05 母板）。**没人产，按需读。**

## 定位

工作流版本内置、固定不变、所有相关角色**只读**的资源。生命周期 = 工作流版本（打 bundle 时固定）。对照 MCP：= **Resources**（应用控制、只读、URI 寻址，`subscribe=false`、`listChanged=false`），但**授权方是 Router 不是环境**（见 `08` §0）。

## 放哪 + 怎么组织

- 物理位置：`ppt-workflow/templates/`（base.css、L01-L05 母板）+ `ppt-workflow/reference/`（design_tokens.md、规范）。bundle 解包到 `bundle/workflow/h3c-ppt/{templates,reference}/`。
- **单一真相源清单 `static_assets.json`**（放 `ppt-workflow/` 根，跟 agent_registry.json / route_table.json 同级——三个并列的 workflow 实例级真相源）。别让路径字符串散在各角色定义里。

```jsonc
{
  "schema_version": "1.0",
  "workflow_version": "h3c_blue_default@1.0.0",
  "assets": {
    "base_css":      { "path": "templates/base.css",        "type": "style",    "audience": ["slide_writer","qa_inspector"],            "inline": false },
    "design_tokens": { "path": "reference/design_tokens.md", "type": "token",    "audience": ["designer","slide_writer","illustrator"],  "inline": "summary" },
    "tpl_L01":       { "path": "templates/L01-cover.html",          "type": "template", "audience": ["slide_writer"], "inline": false },
    "tpl_L02":       { "path": "templates/L02-section-anchor.html", "type": "template", "audience": ["slide_writer"], "inline": false },
    "tpl_L03":       { "path": "templates/L03-title-body.html",     "type": "template", "audience": ["slide_writer"], "inline": false },
    "tpl_L04":       { "path": "templates/L04-bullet-three.html",   "type": "template", "audience": ["slide_writer"], "inline": false },
    "tpl_L05":       { "path": "templates/L05-kpi-hero.html",       "type": "template", "audience": ["slide_writer"], "inline": false }
  }
}
```

字段：
- `path`：相对 workflow 根的资源路径。
- `type`：标签（style/token/template/rule），纯文档/分类用。
- `audience`：哪些角色能用（资产视角，对照 MCP Resource 的 audience annotation）。
- `inline`：注入策略——`false`（只发地址）/ `"summary"`（摘要内联 + 完整版给地址）。见下 §阈值规则。

整个 namespace **物理只读**，所以不需要 writable 字段。

## SubAgent 怎么访问：Router 信封发地址 + read_file 懒加载

遵循 `08` §0 寻址原则。**6 向调研对"地址而非内容"100% 收敛**：

- **不全量注入内容**（把 design_tokens 全文塞信封）：弱模型 context decay 更致命（Salesforce 实测单提示 58%→多提示 35%），token 10x + 注意力稀释。
- **不上 RAG/向量**：base.css/HTML 母板是结构化文件，向量化无意义；本地单机引向量库是过度工程（CrewAI 明确：结构化文件走 read_file 不走向量）。
- **选 Router 发地址 + read_file**：Router 按角色 `reads_static` 把资产**地址清单**放进 Task 信封 `[STATIC]` 段，SubAgent `read_file` 按需读。这 = Anthropic just-in-time retrieval 范式 + §0 "地址发放权在 Router"。

### 阈值规则（inline vs 只发地址）
- **小资产（< ~2000 token，如 design_tokens 摘要、规范要点）**：`inline:"summary"` → harness 把摘要内联进信封 `[STATIC]` 段，完整版给地址让 read_file。弱模型先看摘要、按需读全文，零遗忘风险。
- **大资产（base.css 全文、L01-L05 母板）**：`inline:false` → 只发地址，纯 read_file。

## 怎么声明"角色能用哪些静态资产"（两层互校验）

1. `static_assets.json` 每资产的 `audience`（资产视角：我给谁用）。
2. `agent_registry.json` 每角色新增 `reads_static`（角色视角：我要读啥，引 static_assets 的 key）。
3. CI 对账两者一致（`audience` 含角色 ⟺ 角色 `reads_static` 含该资产），不一致报错。
4. harness spawn 时**只把该角色 reads_static 子集**的地址放进信封，不是全套（最小化注入降弱模型 token 压力）。

## 一致性校验（防漂移）

bundle 编译时对静态资产算 hash 写 `assets_manifest.json`，harness 启动校验。运行期静态资产被改 = 拒绝启动。同时根治 ppt-workflow↔bundle 两份漂移老坑（配合 build.rs BUNDLE_WORKFLOW_HASH）。

## 反模式（已踩坑，红线）

曾让 designer "产出" base.css + DESIGN.md——但这俩在 h3c_blue_default 下就是 `templates/base.css` 和 `reference/design_tokens.md` 的固定内容。让弱模型手写：① 既慢又不符品牌 ② 把单一职责角色搞成"产多种异质输出"（弱模型迷失、调试难）。

**正解**：designer 只产 `page_layout.json`（它真正的决策）；base.css/design_tokens 是静态资产——designer `reads_static:["design_tokens"]` 按规范做布局决策（消费规范，不生成规范）；下游 illustrator/qa 同样 `reads_static` 读 design_tokens 拿设计规范（不再依赖 designer 手写的 DESIGN.md）；base.css 由 `generate_ghost_deck.py` 拷进项目 + 资源目录都在，下游直接用。

> CI 红线（写进 validate_spec.py）：**任何角色 `outputs` 不得出现 `static_assets.json` 登记的 path**——机械堵死这个反模式。
