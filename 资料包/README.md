# pinvou3 客户资料包

面向客户的对外资料（区别于 `docs/` 下的开发内部文档）。

## 已交付：终端用户使用指南

| 文件 | 用途 |
|---|---|
| `pinvou3-使用指南.html` | **网页版**——配 `shots-web/` 一起部署到网站（图片走相对路径，可缓存） |
| `pinvou3-使用指南-单文件.html` | **单文件版**——截图已 base64 内联，发给客户一个文件即可，离线双击就能看（~0.5MB） |
| `shots-web/` | 真实界面截图（webp，网页版引用） |
| `shots/` | 原始截图（中间产物，可删，由 shoot.js 重新生成） |
| `_tooling/` | 截图与构建脚本 |

> 截图说明：用 **mock 数据渲染真实前端代码**（同一个 `pinvou3-app/src/index.html`）得到，界面真实、数据是精心设计的示例。不是手绘示意图。

## 界面改版后如何重出截图

```bash
node 资料包/_tooling/shoot.js      # 渲染真实前端各视图截图 → shots/（需 puppeteer-core，用系统 chromium）
node 资料包/_tooling/optimize.js   # 压成 webp → shots-web/（需 sharp）
node 资料包/_tooling/inline.js     # 生成 base64 单文件版
```

`shoot.js` 注入 mock `__TAURI__`，按命令喂示例数据（会话/卡牌/工具商店/设置等），切换 `currentView` 截图。新增视图或字段对不上时改 `shoot.js` 里的 mock 数据。

## 待做（资料包其余两层）

- **A 决策人**：产品介绍 PDF/PPT，主打"本地算力·数据不外传"。
- **C 客户 IT**：部署运维手册（基于 `pinvou3-app/INSTALL.md` 升级 + 故障排查）。
