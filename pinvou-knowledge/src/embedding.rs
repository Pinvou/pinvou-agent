//! 进程内 BGE-M3 embedding，供桌面本地库与独立服务复用。
//!
//! 有效门控 = `local-embed` feature **且**目标平台存在 fastembed/ort-sys 制品。
//! ort-sys 的 dist 表不含 linux/loongarch64，在该目标上构建期即 panic；Cargo
//! feature 无法按 target 关闭（上层 `local-embed` feature 在 loongarch64 仍然
//! 会开），因此本文件用与 Cargo.toml 依赖表同形的 cfg 复合门控：loongarch64
//! 一律编译下面的显式「平台不支持」桩实现，嵌入请求返回明确错误而不是崩溃。

use std::path::Path;

#[cfg(all(
    feature = "local-embed",
    not(all(target_os = "linux", target_arch = "loongarch64"))
))]
use std::{path::PathBuf, sync::Mutex};

#[cfg(all(
    feature = "local-embed",
    not(all(target_os = "linux", target_arch = "loongarch64"))
))]
use fastembed::{
    InitOptionsUserDefined, Pooling, TextEmbedding, TokenizerFiles, UserDefinedEmbeddingModel,
};

/// 桩实现返回的错误文案：按目标平台二选一，其余语义完全一致。
/// loongarch64（含 feature 开/关两种构建）给出显式的平台不支持；
/// 其余平台的 feature 关闭构建保持历史文案不变。两段 cfg 与下面的
/// 桩 impl 同形（避免常量在真实实现生效的目标上变成 dead_code）。
/// 文案遵循 CONTRIBUTING 的诊断语言约定（英文）：该常量会经 503 映射、
/// model_error 与 document.error 到达 en/ja 用户的界面。
#[cfg(not(all(
    feature = "local-embed",
    not(all(target_os = "linux", target_arch = "loongarch64"))
)))]
#[cfg(all(target_os = "linux", target_arch = "loongarch64"))]
const LOCAL_EMBED_UNAVAILABLE: &str =
    "local embedding is not supported on this platform (linux/loongarch64)";

#[cfg(not(all(
    feature = "local-embed",
    not(all(target_os = "linux", target_arch = "loongarch64"))
)))]
#[cfg(not(all(target_os = "linux", target_arch = "loongarch64")))]
const LOCAL_EMBED_UNAVAILABLE: &str = "local embedding feature disabled";

#[cfg(all(
    feature = "local-embed",
    not(all(target_os = "linux", target_arch = "loongarch64"))
))]
pub struct Embedder {
    model: Mutex<TextEmbedding>,
    name: String,
}

#[cfg(not(all(
    feature = "local-embed",
    not(all(target_os = "linux", target_arch = "loongarch64"))
)))]
pub struct Embedder;

#[cfg(all(
    feature = "local-embed",
    not(all(target_os = "linux", target_arch = "loongarch64"))
))]
impl Embedder {
    pub fn from_dir(dir: &Path, name: &str) -> Result<Self, String> {
        let read = |file: &str| -> Result<Vec<u8>, String> {
            std::fs::read(dir.join(file)).map_err(|error| format!("{file}: {error}"))
        };
        // The directory layout (ONNX candidates and tokenizer files) follows
        // the download manifest's model_layout, so the load path, the
        // download, and the completeness probe share one description.
        let onnx = crate::model_layout::read_first_onnx(dir)?;
        let [
            tokenizer_file,
            config_file,
            special_tokens_map_file,
            tokenizer_config_file,
        ] = crate::model_layout::MODEL_TOKENIZER_FILES;
        let tokenizer_files = TokenizerFiles {
            tokenizer_file: read(tokenizer_file)?,
            config_file: read(config_file)?,
            special_tokens_map_file: read(special_tokens_map_file)?,
            tokenizer_config_file: read(tokenizer_config_file)?,
        };
        let model =
            UserDefinedEmbeddingModel::new(onnx, tokenizer_files).with_pooling(Pooling::Cls);
        let model =
            TextEmbedding::try_new_from_user_defined(model, InitOptionsUserDefined::default())
                .map_err(|error| error.to_string())?;
        Ok(Self {
            model: Mutex::new(model),
            name: name.to_string(),
        })
    }

    pub fn from_env_or_dir(fallback: Option<&Path>) -> Result<Self, String> {
        let dir: PathBuf = std::env::var("PINVOU3_KB_EMBED_MODEL_DIR")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from)
            .or_else(|| fallback.map(Path::to_path_buf))
            .ok_or_else(|| "未配置 embedding 模型目录".to_string())?;
        let name = std::env::var("PINVOU3_KB_EMBED_MODEL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "bge-m3".to_string());
        Self::from_dir(&dir, &name)
            .map_err(|error| format!("embedding 模型加载失败({}): {error}", dir.display()))
    }

    pub fn model(&self) -> &str {
        &self.name
    }

    pub fn source(&self) -> &str {
        "local(fastembed)"
    }

    pub fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let documents: Vec<&str> = texts.iter().map(String::as_str).collect();
        let model = self
            .model
            .lock()
            .map_err(|_| "embedding 模型锁已损坏".to_string())?;
        let mut output = model
            .embed(documents, None)
            .map_err(|error| error.to_string())?;
        for vector in &mut output {
            normalize(vector);
        }
        Ok(output)
    }

    pub fn embed_one(&self, text: &str) -> Result<Vec<f32>, String> {
        self.embed(&[text.to_string()])?
            .pop()
            .ok_or_else(|| "embedding 返回空向量".to_string())
    }
}

#[cfg(not(all(
    feature = "local-embed",
    not(all(target_os = "linux", target_arch = "loongarch64"))
)))]
impl Embedder {
    pub fn from_dir(_dir: &Path, _name: &str) -> Result<Self, String> {
        Err(LOCAL_EMBED_UNAVAILABLE.to_string())
    }

    pub fn from_env_or_dir(_fallback: Option<&Path>) -> Result<Self, String> {
        Err(LOCAL_EMBED_UNAVAILABLE.to_string())
    }

    pub fn model(&self) -> &str {
        "disabled"
    }

    pub fn source(&self) -> &str {
        "local(fastembed disabled)"
    }

    pub fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        Err(LOCAL_EMBED_UNAVAILABLE.to_string())
    }

    pub fn embed_one(&self, _text: &str) -> Result<Vec<f32>, String> {
        Err(LOCAL_EMBED_UNAVAILABLE.to_string())
    }
}

/// 编译期平台能力：本构建是否存在真实（非桩）embedding 实现。cfg 与上面的
/// 桩 impl 同形。宿主服务（server.rs）在启动下载管线前用它拒绝注定无法
/// 加载的下载触发——与桌面端 `local_embed_download_refusal` 是同一语义的
/// 服务端孪生。
#[cfg(not(all(
    feature = "local-embed",
    not(all(target_os = "linux", target_arch = "loongarch64"))
)))]
pub fn local_embed_available() -> bool {
    false
}

#[cfg(all(
    feature = "local-embed",
    not(all(target_os = "linux", target_arch = "loongarch64"))
))]
pub fn local_embed_available() -> bool {
    true
}

/// Whether `error` is this module's stub "platform unsupported" refusal.
/// The HTTP layer maps that refusal to 503 (service unavailable) instead of
/// 400; real-impl builds never produce it, so the answer is always false.
#[cfg(not(all(
    feature = "local-embed",
    not(all(target_os = "linux", target_arch = "loongarch64"))
)))]
pub fn is_local_embed_unavailable(error: &str) -> bool {
    error == LOCAL_EMBED_UNAVAILABLE
}

/// The stub refusal text itself, for gates that must produce exactly the
/// error the HTTP layer maps to 503 (the service-side model load) without
/// routing through an embedder constructor. The download gate deliberately
/// returns its own, more specific copy instead.
#[cfg(not(all(
    feature = "local-embed",
    not(all(target_os = "linux", target_arch = "loongarch64"))
)))]
pub fn local_embed_unavailable_error() -> &'static str {
    LOCAL_EMBED_UNAVAILABLE
}

#[cfg(all(
    feature = "local-embed",
    not(all(target_os = "linux", target_arch = "loongarch64"))
))]
pub fn is_local_embed_unavailable(_error: &str) -> bool {
    false
}

#[cfg(all(
    feature = "local-embed",
    not(all(target_os = "linux", target_arch = "loongarch64"))
))]
fn normalize(vector: &mut [f32]) {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in vector {
            *value /= norm;
        }
    }
}

pub fn vec_to_blob(vector: &[f32]) -> Vec<u8> {
    let mut output = Vec::with_capacity(vector.len() * 4);
    for value in vector {
        output.extend_from_slice(&value.to_le_bytes());
    }
    output
}

pub fn blob_to_vec(blob: &[u8]) -> Vec<f32> {
    blob.as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect()
}

// 桩契约测试：与桩 impl 同形编译（其余平台的 feature 关闭构建 + loongarch64
// 的一切构建），钉住「所有入口都返回显式平台错误、绝不触盘」——该分支此前
// 没有任何 CI 腿编译或运行（knowledge-rust 的 feature-off 步骤运行它）。
#[cfg(not(all(
    feature = "local-embed",
    not(all(target_os = "linux", target_arch = "loongarch64"))
)))]
#[cfg(test)]
mod stub_tests {
    use super::*;

    #[test]
    fn every_entry_point_returns_the_explicit_error_without_touching_disk() {
        assert!(!local_embed_available());
        let error = Embedder::from_dir(Path::new("/nonexistent/pinvou-stub-probe"), "bge-m3")
            .err()
            .expect("stub from_dir must fail");
        assert_eq!(error, LOCAL_EMBED_UNAVAILABLE);
        let error = Embedder::from_env_or_dir(Some(Path::new("/nonexistent/pinvou-stub-probe")))
            .err()
            .expect("stub from_env_or_dir must fail");
        assert_eq!(error, LOCAL_EMBED_UNAVAILABLE);
        let embedder = Embedder;
        assert_eq!(embedder.model(), "disabled");
        assert_eq!(
            embedder.embed(&["x".to_string()]).unwrap_err(),
            LOCAL_EMBED_UNAVAILABLE
        );
        assert_eq!(
            embedder.embed_one("x").unwrap_err(),
            LOCAL_EMBED_UNAVAILABLE
        );
        // The HTTP layer keys its 503 mapping on this predicate; it must
        // recognize exactly the stub refusal that the embedder constructors
        // (from_dir / from_env_or_dir) and the service load path produce
        // (server.rs map_document_write_error). The download gate's refusal
        // is a deliberately distinct copy and is not part of that mapping.
        assert!(is_local_embed_unavailable(LOCAL_EMBED_UNAVAILABLE));
        assert!(!is_local_embed_unavailable("some unrelated load failure"));
    }
}

pub fn cosine(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() || left.is_empty() {
        return -1.0;
    }
    left.iter().zip(right).map(|(a, b)| a * b).sum()
}

// 真实实现方向的镜像钉：fastembed 生效的构建里能力必须为 true，与桩方向
// 的 assert!(!local_embed_available()) 合起来覆盖 2×2 cfg 矩阵。
#[cfg(all(
    feature = "local-embed",
    not(all(target_os = "linux", target_arch = "loongarch64"))
))]
#[cfg(test)]
mod real_impl_tests {
    #[test]
    fn local_embed_is_available_where_fastembed_compiles() {
        assert!(super::local_embed_available());
        // Real-impl builds never produce the stub refusal, so the 503
        // mapping predicate must answer false for any error text.
        assert!(!super::is_local_embed_unavailable("any load failure"));
    }
}
