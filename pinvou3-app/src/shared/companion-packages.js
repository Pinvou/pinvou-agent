// companion 技能 → 所属包 id 的单一真源：工具清单 manifest 的 companion_skills
// 反建。消费方：工具库技能卡路由（ToolStoreView 的 skillToMcp）与场景发送前的
// 可用性自愈（scene-capabilities）——两处必须同一映射，否则 companion 技能的
// 开关/可见性比对会漂移。开关/可见性落盘与读取都是包 id 口径（后端
// to_package_id 归一），技能 id 必须经映射才能跟禁用集/隐藏集比对。
function companionPackageMap(tools) {
  const map = {};
  (tools || []).forEach((tool) => {
    const pkg = String((tool && (tool.id || tool.backendId || tool.skillId)) || '').trim();
    const companions = (tool && (tool.companion_skills || tool.companionSkills)) || [];
    companions.forEach((skillId) => {
      const key = String(skillId || '').trim();
      if (key && pkg) map[key] = pkg;
    });
  });
  return map;
}

export { companionPackageMap };
