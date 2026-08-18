//! 进程内 BGE-M3 embedding，供桌面本地库与独立服务复用。

use std::path::Path;

#[cfg(feature = "local-embed")]
use std::{path::PathBuf, sync::Mutex};

#[cfg(feature = "local-embed")]
use fastembed::{
    InitOptionsUserDefined, Pooling, TextEmbedding, TokenizerFiles, UserDefinedEmbeddingModel,
};

#[cfg(feature = "local-embed")]
pub struct Embedder {
    model: Mutex<TextEmbedding>,
    name: String,
}

#[cfg(not(feature = "local-embed"))]
pub struct Embedder;

#[cfg(feature = "local-embed")]
impl Embedder {
    pub fn from_dir(dir: &Path, name: &str) -> Result<Self, String> {
        let read = |file: &str| -> Result<Vec<u8>, String> {
            std::fs::read(dir.join(file)).map_err(|error| format!("{file}: {error}"))
        };
        let onnx = std::fs::read(dir.join("model.onnx"))
            .or_else(|_| std::fs::read(dir.join("onnx").join("model_int8.onnx")))
            .or_else(|_| std::fs::read(dir.join("onnx").join("model.onnx")))
            .map_err(|error| {
                format!("单文件 ONNX 未找到(model.onnx 或 onnx/model_int8.onnx): {error}")
            })?;
        let tokenizer_files = TokenizerFiles {
            tokenizer_file: read("tokenizer.json")?,
            config_file: read("config.json")?,
            special_tokens_map_file: read("special_tokens_map.json")?,
            tokenizer_config_file: read("tokenizer_config.json")?,
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

#[cfg(not(feature = "local-embed"))]
impl Embedder {
    pub fn from_dir(_dir: &Path, _name: &str) -> Result<Self, String> {
        Err("local embedding feature disabled".to_string())
    }

    pub fn from_env_or_dir(_fallback: Option<&Path>) -> Result<Self, String> {
        Err("local embedding feature disabled".to_string())
    }

    pub fn model(&self) -> &str {
        "disabled"
    }

    pub fn source(&self) -> &str {
        "local(fastembed disabled)"
    }

    pub fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        Err("local embedding feature disabled".to_string())
    }

    pub fn embed_one(&self, _text: &str) -> Result<Vec<f32>, String> {
        Err("local embedding feature disabled".to_string())
    }
}

#[cfg(feature = "local-embed")]
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
    blob.chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

pub fn cosine(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() || left.is_empty() {
        return -1.0;
    }
    left.iter().zip(right).map(|(a, b)| a * b).sum()
}
