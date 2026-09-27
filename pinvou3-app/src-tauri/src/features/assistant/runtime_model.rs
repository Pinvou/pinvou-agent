use crate::platform::prefs::SavedModel;

/// 已完成运行时准备的模型。
///
/// The Community default preparation path is always passthrough: the model is kept as-is
/// and used directly for engine configuration, while credentials still come from
/// environment variables and the local credential store (see engine_pool's
/// `prepare_runtime_model`).
#[derive(Clone, PartialEq, Eq)]
pub struct PreparedRuntimeModel {
    pub model: SavedModel,
}

impl PreparedRuntimeModel {
    pub fn unchanged(model: SavedModel) -> Self {
        Self { model }
    }
}
