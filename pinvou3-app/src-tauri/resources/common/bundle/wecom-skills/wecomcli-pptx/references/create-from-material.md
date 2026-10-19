# 基于材料从零创建PPT

## Step0 · 先确认材料在哪

用户刚发进对话的文件，到手是 **`media_id`**（前缀 `mc`），**不是本地路径**。下方所有本地脚本只吃本地路径，把 `media_id` 直接填进去会以「文件不存在」失败。

按**目的**决定要不要先落地成本地文件：

| 目的 | 需要本地文件 | 怎么做 |
|---|:-:|---|
| **读材料正文**（提炼内容、写大纲） | ❌ | `wecom-cli media extract` 的 `files` 流**直接吃 `media_id`**，一步拿到文本 |
| **转页面图学风格** / **抽材料内嵌图**（要跑本地脚本） | ✅ | **先 `wecom-cli media download` 拿 `file_path`**，再把该路径喂给脚本 |
| **把这个 pptx 导入为在线文档** | ❌ | 导入接口本身支持 `media_id`，**直接传，不要下载** |

### wecom-cli media download — 把 media_id 落地成本地文件

```bash
wecom-cli media download --json '{"media_id": "<MEDIA_ID>"}'
```

| 参数 | 类型 | 必填 | 说明 |
|---|---|:-:|---|
| `media_id` | string | 是 | 媒体文件的唯一标识；只能来自真实返回值，**不可构造或猜测** |

出参 `file_path`（下载文件的本地路径），作为下方脚本的入参。

🔴 **下载产物是链路内部临时副本**：仅供本地脚本读取，用完即弃。**严禁**把该路径、或据此产出的图片 / 中间文件，以附件、本地路径、下载链接等任何形式交付给用户。

> `file_path` 只在**当前执行环境**内有效，不代表用户电脑上的路径。若当前环境无法下载或无法执行本地脚本，本节链路不可用 —— 退回下方文本兜底，不要把宿主机路径冒充可用路径。

## 快速命令（可多选）
| 任务类型 | 命令 |
|---|---|
| 提取上传材料的内容 | `wecom-cli media extract`（直吃 media_id，免下载；本地文件改传 `file_path`） |
| 从材料中提取图片（仅支持docx、pptx、xlsx格式） | `python3 scripts/doc_image_extractor.py "<待提取的文件路径，支持多个文件>" -o "<输出的目录>"` |
| 将上传的pptx文件转换为图片 | `python3 scripts/pptx2image.py "<pptx文件的路径>" --outdir style_preview --grid 9 --dpi 300` |

> 表中的「路径」一律指 Step0 中 `media download` 返回的 `file_path`，或用户直接给出的本地路径。
>
> **读正文一律走 `wecom-cli media extract`**：`media_id` 直吃免下载，本地文件传 `file_path`，服务端解析无需本地装库。

**注意：如果是PDF文件，直接用 `wecom-cli media extract` 读文字内容即可**（pdf 在其支持格式内，且 `media_id` 直吃）。

> **兜底（拿不到页面截图时）**：`pptx2image.py` 的唯一**系统二进制**依赖是 LibreOffice（`soffice`，pptx→pdf）；PDF→PNG 用 `pypdfium2`（纯 pip 包），拼图用 Pillow（pip 包）。若报「soffice not found」，**不要尝试安装这个系统二进制**——pptx 本质是 zip 包，直接对已下载的 `file_path`（Step0 的 `media download` 产物）现场用 python 解析 xml：标准库 `zipfile` 读 `ppt/theme/theme1.xml`（主题色、字体）、`ppt/slides/slideN.xml`（各页布局与文字），挑 3–5 页代表性页面看即可。拿到的是结构化风格参数，直接作为 DESIGN.md 的填色与字体依据。（`pypdfium2` / Pillow 为预装 pip 包；若缺，同样走 xml 解析兜底，**不运行 `pip install`**。）

## 严格遵循约束
- 禁止反复读取或提取材料的内容（`wecom-cli media extract` 对同一份材料只调一次，结果自己留存复用）；
- 可以通过读取pptx转换的图片（或兜底时解析其 xml）去学习pptx文件的风格（包括基本的颜色系统、字体层级、母版与版式、视觉元素，禁止一成不变，模仿无法复现的风格样式）；
- **必须审核从材料中获取的图片质量**，用 `wecomcli-media` 技能的图片解析能力（`wecom-cli media extract`）逐张确认清晰度、完整性与内容。两点注意：
  - **每批最多 5 张**，材料图多于 5 张须拆成多次调用；
  - 该接口对一批图片返回**一段整体文本**，不与图片一一对应。因此必须在 prompt 里显式要求「**逐张按输入顺序分条描述并给出可用性判断**」，或干脆**每次只传 1 张**以保证映射准确。
  
  若材料中图片清晰、完整且满足要求，则在PPT中引用材料中的图片（更符合用户需求）；若图片模糊、不完整或不适用，该图位改用 SVG / 色块 / 版式留白承载（见 [component-image.md](component-image.md) §P1）——**当前环境没有生图工具，不提供 AI 配图**；
- 在高置信度场景（如财务数据分析、学术科研汇报、法律政策解读等）中，不仅PPT的**文本内容**要严格来源于用户上传的材料，**更要优先提取并直接使用材料中原有的图片、图表和数据**。必须保持信息的绝对准确性和一致性；
- ⚠️ **材料图须先上传拿 `url`**（`media_id` 优先直传 `smartpage images upload`，无需落地；本地文件传 `file_path`）；拿到的 `url` 直接写进 JSX 的 `src` 进页面。详见 [component-image.md](component-image.md) §P0。

## 创建流程
材料`内容`和`图片`获取完成之后，请读取并遵循 [create-from-scratch.md](create-from-scratch.md) 中的标准流程来创建PPT。