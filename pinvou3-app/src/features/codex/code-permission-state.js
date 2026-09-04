// code 会话权限模式（Plan/Yolo）的 UI 侧纯逻辑。产品语义（已拍板）：
// 1. 用户从未用过 code 模式时，新建品悟原生 code 会话默认 Plan（只读）；
// 2. 新建 code 会话的默认 mode = code lane 全局 last_mode——只由 code 页
//    **草稿态**显式切换写入；已生成会话的切换只写会话自己的记录，不渗全局
//    （两分 lane 语义：工作/代码各记各的；design lane 已并入 work）；
// 3. 首次切 yolo 弹一次性确认卡（"全自动读写项目目录、可执行 shell、无逐步审批"），
//    确认后全局记住，之后任何会话 Plan↔yolo 切换不再弹。
// 事实源在后端（get_mode_state / get_code_permission_prefs / confirm_code_yolo），
// 这里只放默认值解析与确认门判定，供 CodexAcpView 与测试共用。

/// 无记录 / 读取失败时的兜底 mode：Plan（只读方向是安全侧）。
export const CODE_MODE_FALLBACK = 'plan';

/// 全局默认 mode：code lane 的 last_mode（草稿态显式切换时写入）；无记录
/// （首次使用）→ Plan。
/// prefs 为 get_code_permission_prefs 的返回（{ last_mode, yolo_confirmed }），
/// 可能为 null（尚未拉到或读取失败）。
export function nativeModeFallback(prefs) {
  const last = prefs && typeof prefs.last_mode === 'string' ? prefs.last_mode : null;
  return last === 'plan' || last === 'yolo' ? last : CODE_MODE_FALLBACK;
}

/// 切 yolo 前是否需要弹一次性确认卡。prefs 缺失按未确认处理（安全方向：
/// 宁可多弹一次，不让未确认用户跳过风险提示）。
export function needsYoloConfirmation(prefs) {
  return !(prefs && prefs.yolo_confirmed === true);
}

/// 底栏 mode chip 的展示值：会话控件已按 sessionId 归属刷新 → 用会话实测值；
/// 刷新途中 → 首发物化交接值（handoffMode，无交接则全局默认）；
/// 草稿态 → 全局默认（draft 有用户暂存选择时优先暂存）。
export function resolveNativeModeValue({
  activeId,
  controlsSessionId,
  controlsMode,
  draftMode,
  handoffMode,
  prefs,
}) {
  const fallback = nativeModeFallback(prefs);
  if (activeId && controlsSessionId === activeId && controlsMode) return controlsMode;
  if (activeId && handoffMode) return handoffMode;
  if (activeId) return fallback;
  return draftMode || fallback;
}
