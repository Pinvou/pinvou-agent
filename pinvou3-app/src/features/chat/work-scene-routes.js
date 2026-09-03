import {
  PERSONAL_WORKBENCH_SCENE_KEY,
  createPersonalWorkbenchMessageMeta,
  shouldUsePersonalWorkbenchScene,
} from './personal-workbench-scene.js';

const WORK_DOCUMENT_SCENE_KEY = 'document-writing';
const WORK_DATA_VISUALIZATION_SCENE_KEY = 'data-visualization';
const DESIGN_DATA_VISUALIZATION_SCENE_KEY = WORK_DATA_VISUALIZATION_SCENE_KEY;
const DESIGN_PPT_SCENE_KEY = 'ppt';

const DOCUMENT_WRITING_CONTEXT = `Pinvou 公文写作场景路由：
- 这是强制能力场景，不要按普通聊天或普通 Markdown 文案处理。
- 必须优先加载并使用公文写作技能，技能 id/name 使用 government-writing / 公文写作。
- 如果需要直出可编辑 Word 文件，必须调用公文写作工具，工具 id/name 使用 gongwen / 公文写作。
- 交付目标是规范公文内容或 .docx 产物，不要生成网页、海报、PPT 或通用文章。
- 按党政机关公文习惯组织文种、标题、主送机关、正文层级、落款和日期；缺少关键信息时先给出合理草案并标明可补充项。
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
- deck 必须调 PPT 工具生成，工具 id/name 使用 pptx / PPT 生成（mcp_pptx_make_pptx），slides 数组每页一个对象并按版式填正文字段；按 PPT 内容自动选主题并一句话说明理由。
- 拿到产物路径后必须调用 present_artifact 上产物卡，不要只给文件路径文字。
- 全程不要用 HTML 幻灯片代替 .pptx；没点名在线平台时本地生成，不要用飞书/在线文档代替。
- 如果 pptx 技能或工具不可用，不要静默降级为普通回答或 HTML，应明确提示所需能力不可用。`;

const PPT_DESIGN_AUDIT = `生成完成前执行 PPT 自检：
1. 是否已使用 pptx PPT 生成技能并先给出大纲。
2. 是否已调 mcp_pptx_make_pptx 生成 .pptx 并用 present_artifact 上卡。
3. 每页是否按版式填了真实正文内容，而不是只有标题。
4. 是否避免用 HTML 或在线文档代替本地 .pptx 产物。
如有问题，先自行修正再交付，并在回复中用简短「PPT 自检」说明结果。`;

function shouldUseDocumentWritingScene(mode, subtab) {
  return mode === 'work' && subtab === WORK_DOCUMENT_SCENE_KEY;
}

function shouldUseDataVisualizationScene(mode, subtab) {
  return mode === 'design' && subtab === DESIGN_DATA_VISUALIZATION_SCENE_KEY;
}

function shouldUsePptDesignScene(mode, subtab) {
  return mode === 'design' && subtab === DESIGN_PPT_SCENE_KEY;
}

function buildWorkScenePayloadText(text, context, audit) {
  const raw = String(text || '').trim();
  if (!raw) return raw;
  return `${raw}\n\n---\n${context}\n\n${audit}`;
}

function createDocumentWritingMessageMeta(text) {
  return {
    pinvouScene: `work:${WORK_DOCUMENT_SCENE_KEY}`,
    pinvouRequiredSkill: 'government-writing',
    pinvouRequiredTool: 'gongwen',
    pinvouPayloadText: buildWorkScenePayloadText(text, DOCUMENT_WRITING_CONTEXT, DOCUMENT_WRITING_AUDIT),
  };
}

function createDataVisualizationMessageMeta(text) {
  return {
    pinvouScene: `design:${DESIGN_DATA_VISUALIZATION_SCENE_KEY}`,
    pinvouRequiredSkill: 'visualizer',
    pinvouPayloadText: buildWorkScenePayloadText(text, DATA_VISUALIZATION_CONTEXT, DATA_VISUALIZATION_AUDIT),
  };
}

function createPptDesignMessageMeta(text) {
  return {
    pinvouScene: `design:${DESIGN_PPT_SCENE_KEY}`,
    pinvouRequiredSkill: 'pptx',
    pinvouRequiredTool: 'pptx',
    pinvouPayloadText: buildWorkScenePayloadText(text, PPT_DESIGN_CONTEXT, PPT_DESIGN_AUDIT),
  };
}

export {
  DESIGN_DATA_VISUALIZATION_SCENE_KEY,
  DESIGN_PPT_SCENE_KEY,
  PERSONAL_WORKBENCH_SCENE_KEY,
  WORK_DATA_VISUALIZATION_SCENE_KEY,
  WORK_DOCUMENT_SCENE_KEY,
  buildWorkScenePayloadText,
  createDataVisualizationMessageMeta,
  createDocumentWritingMessageMeta,
  createPersonalWorkbenchMessageMeta,
  createPptDesignMessageMeta,
  shouldUseDataVisualizationScene,
  shouldUseDocumentWritingScene,
  shouldUsePptDesignScene,
  shouldUsePersonalWorkbenchScene,
};
