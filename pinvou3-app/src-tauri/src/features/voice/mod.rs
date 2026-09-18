mod platform;
mod temp_wav;
mod transcript;
pub(crate) mod voice_asr;

/// 麦克风权限重置命令域。实现在私有的 platform 适配层,命令层经此窄口
/// 转发(原 microphone_permission.rs 纯转发微文件已并入此处)。
pub(crate) mod microphone_permission {
    pub use super::platform::reset_microphone_permission;
}

pub(crate) use platform::{
    asr_dependency_packages, asr_missing_message, asr_tool_exists, asr_tool_path,
    engine_binary_name, native_recognition_source, recognize_native,
};
pub(crate) use temp_wav::VoiceTempWav;
pub(crate) use transcript::parse_asr_transcript;
pub(crate) use voice_asr::set_bundled_engine_dir;
