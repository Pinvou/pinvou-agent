use crate::platform::prefs::SavedModel;

/// 已完成运行时准备的模型。
///
/// Community 默认准备路径固定 passthrough：模型原样保留、直接用于引擎配置，
/// 凭据照常走环境变量与本地凭据库（见 engine_pool 的 `prepare_runtime_model`）。
#[derive(Clone, PartialEq, Eq)]
pub struct PreparedRuntimeModel {
    pub model: SavedModel,
}

impl PreparedRuntimeModel {
    pub fn unchanged(model: SavedModel) -> Self {
        Self { model }
    }
}
