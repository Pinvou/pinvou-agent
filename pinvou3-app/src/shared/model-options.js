function visibleUserModels(models) {
  return (models || []).filter(model => model && model.id);
}

export { visibleUserModels };
