// 场景路由载荷 → 用户原文的互逆解析。
//
// 发送路径把用户原文包进只给模型看的路由文案（meta.pinvouPayloadText），气泡显示
// 的是原文；落盘的 user 消息却是包装后的全文。历史重放和远程准入拿不到发送时的
// 原文，只能从载荷反解，否则气泡里带着整段场景提示词。
//
// 分两级，宁可少剥也不吞用户内容：
// - 精确互逆：载荷逐字节等于构造函数对某段原文的输出。除「用户自己把整段载荷
//   粘贴进来」外零误判，且那种情况剥掉的也只是最外层——不需要任何旁证。
// - 旧版本文案兜底：路由正文随版本改过，升级前保存的会话逐字节对不上。各场景
//   包装的首行标题自引入起未变，按标题锚定剥离。锚点取最后一次出现：真包装在
//   落盘文本的尾部，标题若同时出现在用户原文里，取首次会把锚点之后（含用户
//   后续输入）整段吞掉；锚点后还要求紧跟路由正文的「- 」bullet 行，sidecar
//   位置漂移到相邻手打消息上时，手敲正文几乎不会长这样。这仍是启发式，只在
//   场景事件 sidecar 确认该消息是本场景发送时启用。
//
// 另有一类复合形态：发送路径把规范提及块（session-mention.js 的机器契约）拼在
// 载荷头部。后缀式包装对任意前缀天然免疫；前缀式的工作台则先剥块再走同一两级
// 判定，块原样留在还原结果里交还气泡解析（与后缀式复合同一形状）。
//
// 沿用 session-mention.js 的 window 全局约定发布给 classic-script 桥：桥不能反向
// import features，解析的单一事实源留在这里。
import {
  PERSONAL_WORKBENCH_SCENE_KEY,
  POSTER_SCENE_KEY,
  pinvouSceneTag,
} from './scene-registry.js';
import {
  SCENE_PAYLOAD_SEPARATOR,
  SUFFIX_SCENE_ROUTES,
  scenePayloadSuffix,
} from './work-scene-routes.js';
import { VISUAL_POSTER_AUDIT, VISUAL_POSTER_CONTEXT } from './visual-poster-scene.js';
import {
  DEFAULT_PERSONAL_WORKBENCH_PROMPT,
  PERSONAL_WORKBENCH_REQUEST_MARK,
} from './personal-workbench-scene.js';
import { splitSessionMentionBlock } from './session-mention.js';

// 四个「原文 + 分隔线 + 路由正文 + 自检」的后缀式场景。
const SUFFIX_ROUTES = [
  ...SUFFIX_SCENE_ROUTES,
  { key: POSTER_SCENE_KEY, context: VISUAL_POSTER_CONTEXT, audit: VISUAL_POSTER_AUDIT },
];

// `allowHeaderAnchor` 关闭时只认逐字节互逆；打开后才按首行标题锚定旧版本文案。
function withoutTrailingNewlines(value) {
  let end = value.length;
  while (end > 0 && (value[end - 1] === '\n' || value[end - 1] === '\r')) end -= 1;
  return end === value.length ? value : value.slice(0, end);
}

function splitSuffixRoute(payload, route, allowHeaderAnchor) {
  const suffix = scenePayloadSuffix(route.context, route.audit);
  if (payload.length > suffix.length && payload.endsWith(suffix)) {
    return payload.slice(0, -suffix.length);
  }
  // 引擎或持久化可能在载荷尾部追加换行：对去掉尾随换行的副本再比一次。能多剥
  // 的只有包装之外的空白，不是用户内容；正文为空的裸包装仍然不算载荷。
  const trimmed = withoutTrailingNewlines(payload);
  if (trimmed !== payload && trimmed.length > suffix.length && trimmed.endsWith(suffix)) {
    return trimmed.slice(0, -suffix.length);
  }
  if (!allowHeaderAnchor) return null;
  const anchor = `${SCENE_PAYLOAD_SEPARATOR}${route.context.split('\n')[0]}\n`;
  // 取最后一次出现：包装总在尾部，用户原文里若也出现锚点样式，取首次会把
  // 锚点之后的全部原文吞掉。锚点后要求紧跟「- 」bullet（各场景路由正文的首行
  // 约定），手敲同款标题的后续正文几乎不会长这样——sidecar 漂移到相邻手打
  // 消息上时不再误吞，宁可少剥。
  const index = payload.lastIndexOf(anchor);
  if (index <= 0) return null;
  if (!payload.startsWith('- ', index + anchor.length)) return null;
  return payload.slice(0, index);
}

function splitWorkbenchBody(payload, allowHeaderAnchor) {
  const prefix = `${DEFAULT_PERSONAL_WORKBENCH_PROMPT}${PERSONAL_WORKBENCH_REQUEST_MARK}`;
  if (payload.length > prefix.length && payload.startsWith(prefix)) {
    return payload.slice(prefix.length);
  }
  if (!allowHeaderAnchor) return null;
  // 首句自场景引入起未变；当前提示词逐字节命中时不依赖它。标记取第一次出现：
  // 提示词在前、用户原文在后，原文里再出现「用户需求：」不影响切分点。
  const head = `${DEFAULT_PERSONAL_WORKBENCH_PROMPT.split('。')[0]}。`;
  const index = payload.indexOf(PERSONAL_WORKBENCH_REQUEST_MARK);
  if (!payload.startsWith(head) || index < head.length) return null;
  return payload.slice(index + PERSONAL_WORKBENCH_REQUEST_MARK.length) || null;
}

function splitWorkbenchRoute(payload, allowHeaderAnchor) {
  const body = splitWorkbenchBody(payload, allowHeaderAnchor);
  if (body !== null) return body;
  // 复合形态：块 + 载荷。matched 只认头部结构规范的块（解析器的容错把手写
  // 仿品原样交还）；仅引用无正文的块（text 为空）与块后跟不出的载荷都不剥，
  // 消息永不为空。
  const mention = splitSessionMentionBlock(payload);
  if (!mention.matched || !mention.text) return null;
  const inner = splitWorkbenchBody(mention.text, allowHeaderAnchor);
  if (inner === null) return null;
  return payload.slice(0, payload.length - mention.text.length) + inner;
}

function splitStaleRoute(payload, sceneHint) {
  const suffixRoute = SUFFIX_ROUTES.find(route => pinvouSceneTag(route.key) === sceneHint);
  const workbenchScene = pinvouSceneTag(PERSONAL_WORKBENCH_SCENE_KEY);
  const body = suffixRoute
    ? splitSuffixRoute(payload, suffixRoute, true)
    : (sceneHint === workbenchScene ? splitWorkbenchRoute(payload, true) : null);
  return body === null ? null : { scene: sceneHint, text: body };
}

/**
 * 把落盘的场景载荷还原成用户原文。
 * @param {string} payload 落盘 user 消息的全文
 * @param {string} [sceneHint] 场景事件 sidecar 记在该消息位置的场景标签
 * @returns {{ scene: string, text: string } | null} 不是可识别的场景载荷时为 null
 */
export function splitScenePayload(payload, sceneHint = '') {
  if (!payload) return null;

  const exact = [];
  for (const route of SUFFIX_ROUTES) {
    const body = splitSuffixRoute(payload, route, false);
    if (body !== null) exact.push({ scene: pinvouSceneTag(route.key), text: body });
  }
  const workbench = splitWorkbenchRoute(payload, false);
  if (workbench !== null) {
    exact.push({ scene: pinvouSceneTag(PERSONAL_WORKBENCH_SCENE_KEY), text: workbench });
  }
  // 用户把别的场景载荷整段粘贴进来时，头尾可能各挂一层包装：sidecar 记账的
  // 场景决定哪一层是最外层。没有记账而两级都命中时，优先工作台读法——把场景
  // 载荷粘进工作台输入框发送是最常见的复合形态，工作台（前缀式）读法逐字节
  // 还原原文；后缀式读法会把工作台提示词当成「原文」漏进气泡。
  const hinted = exact.find(match => match.scene === sceneHint);
  if (hinted) return hinted;
  const workbenchHit = exact.find(match => match.scene === pinvouSceneTag(PERSONAL_WORKBENCH_SCENE_KEY));
  return workbenchHit || exact[0] || splitStaleRoute(payload, sceneHint);
}

if (typeof window !== 'undefined') {
  window.__PINVOU_SCENE_PAYLOAD__ = { splitScenePayload };
}
