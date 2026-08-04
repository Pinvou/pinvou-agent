function visibleUserModels(models) {
  return (models || []).filter(model => model && model.id);
}

function modelDisplayName(model) {
  return (model && (model.model || model.name)) || '';
}

export { modelDisplayName, visibleUserModels };
