// ensure_folder_projects 结果解释(§9.9 文件夹通道):选择器浏览通道与两条
// 车道的"最近目录"通道共用同一份判定,避免各处重复实现而漂移。
// - created/covered:所选文件夹已拥有锚定项目(新建或复用),提取其项目 id
//   (created 带 project 对象,covered 带 project_id);会话必须归属到该项目,
//   否则 tier-2 嵌套归组会把子目录会话收养进根覆盖它的宽项目(如 Desktop)。
// - 无 outcome(真正空列表):命中反物化排除表(§3),如实按普通文件夹处理。
// - failed:后端对该根明确拒绝(嵌套冲突等)——不是排除表,调用方应告知用户
//   而不是静默继续。IPC 级异常不进此函数,由调用方 catch。
// round-8 m14:排除表解读只属于真正空列表。created/covered 缺项目 id 时
// tier-① 归属无从携带,按 materialized=true 放行会静默跳过归属、让 tier-②
// 收养;未知 status 同理——两者都按 failed 上抛,绝不冒充排除表。
export function interpretFolderEnsureOutcomes(outcomes) {
  const list = Array.isArray(outcomes) ? outcomes : [];
  const hit = list.find(o => o && (o.status === 'created' || o.status === 'covered'));
  const projectId = hit
    ? (hit.status === 'created' ? (hit.project && hit.project.id) : hit.project_id) || null
    : null;
  const usable = !!hit && projectId !== null;
  const failed = list.length > 0
    && (!usable || list.some(o => o && o.status === 'failed'));
  return {
    materialized: usable,
    projectId: usable ? projectId : null,
    failed,
  };
}
