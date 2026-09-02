const VISUAL_POSTER_SCENE_KEY = 'poster';

const VISUAL_POSTER_CONTEXT = `Pinvou 视觉海报场景约束：
- 交付物优先生成自包含 HTML/CSS 视觉海报或 Banner，保证可以在 Pinvou 产物预览里继续点选、编辑文字和样式。
- 构图必须有一个明确视觉焦点，用户第一眼能看出主标题或主视觉；不要让 3 个以上元素竞争焦点。
- 先建立阅读路径：主视觉/主标题 -> 副标题 -> 关键信息 -> 行动入口/落款。
- 主视觉优先使用真实图片、真实产品图、真实场景图或真实质感素材；不要只靠渐变、抽象 SVG、装饰纹理撑完整画面。
- 如果当前环境具备联网、图片搜索、下载或写文件能力，必须优先检索并下载与主题贴合的真实图片，保存到当前工作区后用本地相对路径引用；也可以使用稳定可访问的图片 URL。
- 图片必须服务主题和文案，不要使用无关风景、抽象背景或通用图库图凑数；关键文案仍保持 live text，不要烘焙进图片。
- 只有在无法联网、无法搜索或无法下载图片时，才允许降级为可替换图片占位、真实质感背景和明确图片尺寸/位置，并在回复中说明能力限制。
- 字体控制在 1-2 套以内，通过字号、字重、位置建立层级；关键文字保持 live text，不要烘焙进图片。
- 色彩角色控制在 4 个以内：主色、辅助色、强调色、文字/中性色；避免无目的多色装饰。
- 保留清晰留白和边距，信息太多时删减内容，不要靠压缩字号解决。
- 如果用户指定海报、活动海报、电商 Banner、产品发布海报，优先按固定画布比例设计，不要做普通网页落地页。`;

const VISUAL_POSTER_AUDIT = `生成完成前执行海报自检：
1. 是否只有一个最强视觉焦点。
2. 标题、副标题、关键信息是否有明显层级。
3. 是否存在过小、难读或被裁切的文字。
4. 字体是否不超过 2 套，颜色角色是否不超过 4 个。
5. 是否已经尝试联网检索/下载贴合主题的真实图片；如果没有，是否说明能力限制。
6. 是否使用了合适的真实图片/真实质感主视觉；如果没有，是否提供了可替换图片位置或说明原因。
7. 关键文案是否仍是可编辑 HTML 文本。
8. 画面是否有足够留白，没有把内容塞满整张画布。
如有问题，先自行修正再交付，并在回复中用简短「海报自检」说明结果。`;

function shouldUseVisualPosterScene(mode, subtab) {
  return mode === 'design' && subtab === VISUAL_POSTER_SCENE_KEY;
}

function buildVisualPosterPayloadText(text) {
  const raw = String(text || '').trim();
  if (!raw) return raw;
  return `${raw}\n\n---\n${VISUAL_POSTER_CONTEXT}\n\n${VISUAL_POSTER_AUDIT}`;
}

function createVisualPosterMessageMeta(text) {
  return {
    pinvouScene: `design:${VISUAL_POSTER_SCENE_KEY}`,
    pinvouDesignScene: VISUAL_POSTER_SCENE_KEY,
    pinvouPayloadText: buildVisualPosterPayloadText(text),
  };
}

export {
  VISUAL_POSTER_SCENE_KEY,
  buildVisualPosterPayloadText,
  createVisualPosterMessageMeta,
  shouldUseVisualPosterScene,
};
