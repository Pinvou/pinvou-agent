function isBuiltInModelOption(model) {
  if (!model) return false;
  const id = String(model.id || '').trim().toLowerCase();
  const name = String(model.name || '').trim().toLowerCase();
  const wireModel = String(model.model || '').trim().toLowerCase();
  return id === 'builtin_llmapi' ||
    id === 'builtin-model' ||
    name === '内置模型' ||
    name === 'built-in model' ||
    name === 'builtin model' ||
    wireModel === '内置模型' ||
    wireModel === 'built-in model' ||
    wireModel === 'builtin model';
}

function visibleUserModels(models) {
  return (models || []).filter(model => model && model.id && !isBuiltInModelOption(model));
}

export { isBuiltInModelOption, visibleUserModels };
