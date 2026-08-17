# OCR 图片识别参考文档

## 工具总览

| 工具 | 功能 | 输入 | 输出 |
|------|------|------|------|
| `ocr.extract` | 识别单张图片文字 | 单张图片 | 文字列表，可选带坐标 |
| `ocr.toword` | 图片转在线文档 | 1-9 张图片 | `file_id` + `file_url` |
| `ocr.toexcel` | 图片表格转在线表格 | 1-9 张图片 | `file_id` + `file_url` |

**限制**：单张 ≤10MB，总 ≤50MB，格式 PNG/JPG/JPEG/BMP/WEBP

## 图片来源路由（重要）

```
├─ 有公网 URL → 直接调 ocr.* 工具，填 image_url（首选）
├─ 本地文件   → node ocr.js（禁止手动传 base64）
└─ data URI   → 先存本地文件，再走 ocr.js
```

**本地图片禁止将 base64 作为工具参数传入**，LLM 无法处理超长字符串。使用 `ocr.js` 脚本（自动编码+调用）：

```bash
node ocr.js extract /path/to/image.png [--accurate|--efficient] [--positions]
node ocr.js toword  /path/to/p1.png /path/to/p2.png [--title "标题"]
node ocr.js toexcel /path/to/table.png [--title "标题"]
```

## 图片输入字段规则

`image_url` 与 `image_base64` **严格二选一**，不能同时填也不能都不填：
- `image_url`：公网 http(s) URL，必须后端可直接下载（不支持内网/需鉴权/过期签名地址）
- `image_base64`：纯 base64 字符串，**不接受** URL 或 `data:image/...;base64,` 前缀

---

## ocr.extract

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `image_url` | string | 二选一（首选） | 公网图片 URL |
| `image_base64` | string | 二选一 | 纯 base64 字符串 |
| `extract_type` | string | 否 | `basic`（默认，平衡）/ `accurate`（高精度，适合小字模糊）/ `efficient`（快速） |
| `with_positions` | bool | 否 | 是否返回文字坐标，默认 false |

**返回**：`texts`(string[]) 文字列表 + `text_detections`(仅 with_positions=true 时) 带坐标结果

```json
{"image_url": "https://example.com/invoice.png", "extract_type": "accurate", "with_positions": true}
```

## ocr.toword / ocr.toexcel

两个工具参数结构相同，区别仅在输出类型（文档 vs 表格）。单张图片时启用矫正增强，效果优于批量。

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `images` | array | 是 | 1-9 张，每项含 `image_url` 或 `image_base64` 二选一 |
| `title` | string | 否 | 标题，默认"OCR识别文档"/"OCR识别表格" |

**返回**：`file_id` + `file_url`

```json
{"images": [{"image_url": "https://example.com/page-1.png"}], "title": "会议纪要"}
```

---

## 典型工作流

### 提取图片文字
1. URL → `ocr.extract`；本地 → `node ocr.js extract <path>`
2. 从 `texts` 拼接结果反馈用户

### 图片转文档/表格
1. URL → `ocr.toword`/`ocr.toexcel`；本地 → `node ocr.js toword|toexcel <paths>`
2. 返回 `file_url` 给用户

### OCR 回填到现有文档
1. 先用上述方式拿到 `texts`
2. 按目标类型写回：smartcanvas → `smartcanvas.edit`(INSERT_AFTER) / Word → `insert_markdown` / sheet → `smartsheet.add_records`

---

## 注意事项

- 同步接口，图片多或精度高时较慢，耐心等待不要重复触发
- 仅 1 张图且对质量敏感时，不要凑数传多张（单张有矫正增强）
- URL 下载失败时改用 base64 重试
