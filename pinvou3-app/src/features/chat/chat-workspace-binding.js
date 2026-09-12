// 普通聊天「绑定工作目录会话」相关 UI 纯逻辑（ChatView 与测试共用，
// 对齐 features/codex/code-permission-state.js 的抽取模式）：
// 绑定会话/草稿的安全姿态对齐 code 模式——切 YOLO 前过一次性确认门、
// composer 旁显示绑定目录指示。确认门本身的判定复用 codex 侧的
// needsYoloConfirmation（同一后端事实源 get_code_permission_prefs）。

/// 门控绑定解析的"未知"哨兵：绑定查询瞬时失败（非旧后端缺命令）时，
/// ChatView 返回此非空值让确认门 fail-closed 过量施加一次确认，优于对
/// 已绑定会话静默跳过（评审 #445 R3）。仅参与真值判定，不展示、不缓存。
export const CHAT_YOLO_GATE_UNKNOWN_BINDING = '(binding-query-failed)';

/// 切 YOLO 前是否需要走确认门检查：已生成会话看其目录绑定，草稿看
/// draftWorkspacePath。返回 true 仅表示「需要查 prefs 判定」，是否真弹卡
/// 由 needsYoloConfirmation(prefs) 决定（已确认过 yolo_confirmed=true 不弹）。
export function chatYoloGateApplies({ activeSessionId, sessionBinding, draftWorkspacePath }) {
  return activeSessionId ? !!sessionBinding : !!draftWorkspacePath;
}

/// composer 旁绑定目录 chip 的显示条件：仅活动会话且查询到绑定路径
/// （查询失败/无绑定/Web 端桩返回 null 一律不显示）。
export function shouldShowWorkspaceBindingChip({ activeSessionId, sessionBinding }) {
  return !!activeSessionId && !!sessionBinding;
}
