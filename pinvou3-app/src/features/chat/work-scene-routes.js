import {
  DATA_VISUALIZATION_SCENE_KEY,
  DOCUMENT_WRITING_SCENE_KEY,
  PPT_DESIGN_SCENE_KEY,
  pinvouSceneTag,
} from './scene-registry.js';

const DOCUMENT_WRITING_CONTEXT = `Pinvou 公文写作场景路由：
- 这是强制能力场景，不要按普通聊天或普通 Markdown 文案处理。
- 必须优先加载并使用公文写作技能，技能 id/name 使用 government-writing / 公文写作。
- 如果需要直出可编辑 Word 文件，必须调用公文写作工具，工具 id/name 使用 gongwen / 公文写作。
- 交付目标是规范公文内容或 .docx 产物，不要生成网页、海报、PPT 或通用文章。
- 按党政机关公文习惯组织文种、标题、主送机关、正文层级、落款和日期；缺少关键信息时先给出合理草案并标明可补充项。
- 如果 tool_search 没搜到目标工具（gongwen）且结果带 mcp_boot connecting 状态（servers_pending 列出仍在启动的 MCP 服务器），说明工具服务仍在启动、目标工具只是暂不可搜：这是临时状态而不是能力缺失——等待数秒重试 tool_search，仍返回 connecting 就继续等待并再次重试；若多轮重试后仍是 connecting，告知用户工具服务仍在启动、请稍后重试，不要按能力不可用处理；connecting 状态消失后仍搜不到，才按能力不可用处理。
- 如果 government-writing 技能或 gongwen 工具不可用，不要静默降级为普通回答，应明确提示所需能力不可用。`;

const DOCUMENT_WRITING_AUDIT = `生成完成前执行公文自检：
1. 是否已使用 government-writing 公文写作技能。
2. 需要 .docx 时是否已使用 gongwen 公文写作工具。
3. 文种、标题、正文层级、落款和日期是否完整。
4. 是否避免生成网页、海报、PPT 或通用散文。
如有问题，先自行修正再交付，并在回复中用简短「公文自检」说明结果。`;

const DATA_VISUALIZATION_CONTEXT = `Pinvou 数据可视化场景路由：
- 这是强制能力场景，不要按普通聊天、Excel 仪表盘或泛化可视化处理。
- 必须优先加载并使用数据分析可视化技能，技能 id/name 使用 visualizer / 数据分析可视化。
- 交付目标是可在 Pinvou 产物预览中打开的 HTML 可视化仪表盘，默认使用 Chart.js。
- 可以做 KPI 卡、趋势图、对比图、分布图、表格摘要和结论区，但不要输出 Excel 仪表盘、PPT 或普通 Markdown 报告。
- 数据不足时先基于用户描述构造清晰的示例数据，并在回复中说明示例假设；用户提供真实数据时必须优先使用真实数据。
- 如果 visualizer 技能不可用，不要静默降级为普通回答，应明确提示所需能力不可用。`;

const DATA_VISUALIZATION_AUDIT = `生成完成前执行数据可视化自检：
1. 是否已使用 visualizer 数据分析可视化技能。
2. 是否交付 HTML + Chart.js 可视化产物。
3. 是否避免输出 Excel 仪表盘、PPT 或普通 Markdown 报告。
4. 图表标题、指标口径、图例、结论是否清晰。
如有问题，先自行修正再交付，并在回复中用简短「可视化自检」说明结果。`;

const PPT_DESIGN_CONTEXT = `Pinvou PPT 设计场景路由：
- 这是强制能力场景，不要按普通聊天、网页或 Markdown 大纲处理。
- 必须优先加载并使用 PPT 生成技能，技能 id/name 使用 pptx / PPT 生成。
- 交付目标是可编辑的 .pptx 文件：先列一版大纲（章节 + 每页要点）给用户确认，确认后产结构化 deck。
- deck 必须调 PPT 工具生成，工具 id/name 使用 pptx / PPT 生成（mcp_pptx_make_pptx；连接器工具默认延迟加载，不在工具列表时先 tool_search 激活），slides 数组每页一个对象并按版式填正文字段；按 PPT 内容自动选主题并一句话说明理由。
- 如果 tool_search 没搜到目标工具（mcp_pptx_make_pptx、mcp_pinvou3_present_artifact）且结果带 mcp_boot connecting 状态（servers_pending 列出仍在启动的 MCP 服务器），说明工具服务仍在启动、目标工具只是暂不可搜：这是临时状态而不是能力缺失——等待数秒重试 tool_search，仍返回 connecting 就继续等待并再次重试；若多轮重试后仍是 connecting，告知用户工具服务仍在启动、请稍后重试，不要按能力不可用处理；connecting 状态消失后仍搜不到，才按能力不可用处理。
- 拿到产物路径后必须调用 mcp_pinvou3_present_artifact 上产物卡（内置 MCP 工具，默认延迟加载：工具列表里没有它时先 tool_search 激活；tool_search 返回 mcp_boot connecting（MCP 服务仍在启动）就等待数秒重试，connecting 消失后仍搜不到，才说明产物卡后端不可用，交付文件并明确告知用户），不要只给文件路径文字。
- 全程不要用 HTML 幻灯片代替 .pptx；没点名在线平台时本地生成，不要用飞书/在线文档代替。
- 如果 pptx 技能或工具不可用，不要静默降级为普通回答或 HTML，应明确提示所需能力不可用。`;

const PPT_DESIGN_AUDIT = `生成完成前执行 PPT 自检：
1. 是否已使用 pptx PPT 生成技能并先给出大纲。
2. 是否已调 mcp_pptx_make_pptx 生成 .pptx 并用 mcp_pinvou3_present_artifact 上卡（tool_search 激活后仍不可用时是否已明确告知产物卡不可用）。
3. 每页是否按版式填了真实正文内容，而不是只有标题。
4. 是否避免用 HTML 或在线文档代替本地 .pptx 产物。
如有问题，先自行修正再交付，并在回复中用简短「PPT 自检」说明结果。`;

function shouldUseDocumentWritingScene(subtab) {
  return subtab === DOCUMENT_WRITING_SCENE_KEY;
}

function shouldUseDataVisualizationScene(subtab) {
  return subtab === DATA_VISUALIZATION_SCENE_KEY;
}

function shouldUsePptDesignScene(subtab) {
  return subtab === PPT_DESIGN_SCENE_KEY;
}

function buildWorkScenePayloadText(text, context, audit) {
  const raw = String(text || '').trim();
  if (!raw) return raw;
  return `${raw}\n\n---\n${context}\n\n${audit}`;
}

function createDocumentWritingMessageMeta(text) {
  return {
    pinvouScene: pinvouSceneTag(DOCUMENT_WRITING_SCENE_KEY),
    pinvouRequiredSkill: 'government-writing',
    pinvouRequiredTool: 'gongwen',
    pinvouPayloadText: buildWorkScenePayloadText(text, DOCUMENT_WRITING_CONTEXT, DOCUMENT_WRITING_AUDIT),
  };
}

function createDataVisualizationMessageMeta(text) {
  return {
    pinvouScene: pinvouSceneTag(DATA_VISUALIZATION_SCENE_KEY),
    pinvouRequiredSkill: 'visualizer',
    pinvouPayloadText: buildWorkScenePayloadText(text, DATA_VISUALIZATION_CONTEXT, DATA_VISUALIZATION_AUDIT),
  };
}

function createPptDesignMessageMeta(text) {
  return {
    pinvouScene: pinvouSceneTag(PPT_DESIGN_SCENE_KEY),
    pinvouRequiredSkill: 'pptx',
    pinvouRequiredTool: 'pptx',
    pinvouPayloadText: buildWorkScenePayloadText(text, PPT_DESIGN_CONTEXT, PPT_DESIGN_AUDIT),
  };
}

export {
  createDataVisualizationMessageMeta,
  createDocumentWritingMessageMeta,
  createPptDesignMessageMeta,
  shouldUseDataVisualizationScene,
  shouldUseDocumentWritingScene,
  shouldUsePptDesignScene,
};
