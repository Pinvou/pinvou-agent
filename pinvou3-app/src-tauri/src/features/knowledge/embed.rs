//! L1 向量化：fastembed-rs 进程内 ONNX 推理（CPU EP），**不依赖 vLLM**。
//!
//! 模型(bge-m3 ONNX)由用户本地下载，[`Embedder::from_dir`] 用
//! `UserDefinedEmbeddingModel` 从目录直接加载，不走 HuggingFace 自动下载。
//! 向量化只在后台 `std::thread`（解析/检索线程）里调用，避免阻塞 async runtime。
//! 端点/模型未配置（env 缺失或加载失败）时上层退回全文 fts 检索。

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
    /// 从本地模型目录加载。目录需含：`model.onnx`（发布布局）或
    /// `onnx/model_int8.onnx`（Hugging Face 原始布局）+
    /// `tokenizer.json` / `config.json` / `special_tokens_map.json` / `tokenizer_config.json`。
    pub fn from_dir(dir: &Path, name: &str) -> Result<Self, String> {
        let read = |f: &str| -> Result<Vec<u8>, String> {
            std::fs::read(dir.join(f)).map_err(|e| format!("{f}: {e}"))
        };
        // fastembed 通过 commit_from_memory 加载 ONNX，无法解析 split ONNX 的
        // model.onnx_data；原始模型目录必须优先选自包含的 model_int8.onnx。
        let onnx = std::fs::read(dir.join("model.onnx"))
            .or_else(|_| std::fs::read(dir.join("onnx").join("model_int8.onnx")))
            .or_else(|_| std::fs::read(dir.join("onnx").join("model.onnx")))
            .map_err(|e| format!("单文件 ONNX 未找到(model.onnx 或 onnx/model_int8.onnx): {e}"))?;
        let tokenizer_files = TokenizerFiles {
            tokenizer_file: read("tokenizer.json")?,
            config_file: read("config.json")?,
            special_tokens_map_file: read("special_tokens_map.json")?,
            tokenizer_config_file: read("tokenizer_config.json")?,
        };
        // bge-m3 dense 用 CLS pooling；输出再手动归一化(便于点积即余弦)。
        let udm = UserDefinedEmbeddingModel::new(onnx, tokenizer_files).with_pooling(Pooling::Cls);
        crate::platform::os::configure_onnxruntime_dylib()?;
        let model =
            TextEmbedding::try_new_from_user_defined(udm, InitOptionsUserDefined::default())
                .map_err(|e| e.to_string())?;
        Ok(Self {
            model: Mutex::new(model),
            name: name.to_string(),
        })
    }

    /// 定位模型目录并加载。优先 env `PINVOU3_KB_EMBED_MODEL_DIR`(开发用 run-dev.sh 设);
    /// env 缺失则用 `fallback`。名称 env `PINVOU3_KB_EMBED_MODEL`(默认 bge-m3)。
    /// 保留具体加载错误交给服务层记录/返回，避免把“文件已安装但运行时加载失败”误报成未配置。
    pub fn from_env_or_dir(fallback: Option<&Path>) -> Result<Self, String> {
        let dir: PathBuf = std::env::var("PINVOU3_KB_EMBED_MODEL_DIR")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
            .or_else(|| fallback.map(|p| p.to_path_buf()))
            .ok_or_else(|| "未配置 embedding 模型目录".to_string())?;
        let name = std::env::var("PINVOU3_KB_EMBED_MODEL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "bge-m3".into());
        Self::from_dir(&dir, &name)
            .map_err(|err| format!("embedding 模型加载失败({}): {err}", dir.display()))
    }

    pub fn model(&self) -> &str {
        &self.name
    }
    /// 给 embed_info 复用（进程内本地推理，无 URL）。
    pub fn source(&self) -> &str {
        "local(fastembed)"
    }

    /// 批量编码，返回**归一化**后的向量（点积即余弦）。
    pub fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        let docs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
        let m = self
            .model
            .lock()
            .map_err(|_| "embed lock poisoned".to_string())?;
        let mut out = m.embed(docs, None).map_err(|e| e.to_string())?;
        for v in out.iter_mut() {
            normalize(v);
        }
        Ok(out)
    }

    pub fn embed_one(&self, text: &str) -> Result<Vec<f32>, String> {
        let mut v = self.embed(&[text.to_string()])?;
        v.pop().ok_or_else(|| "empty embedding".to_string())
    }
}

#[cfg(not(feature = "local-embed"))]
impl Embedder {
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
fn normalize(v: &mut [f32]) {
    let n: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        for x in v.iter_mut() {
            *x /= n;
        }
    }
}

/// f32 向量 ↔ BLOB（little-endian）。
pub fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}
pub fn blob_to_vec(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// 余弦相似度（两个已归一化向量 → 点积）。
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return -1.0;
    }
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实模型冒烟（默认 #[ignore]，下载 bge-m3 ONNX 后在该机跑）：
    ///   PINVOU3_KB_EMBED_MODEL_DIR=~/models/bge-m3 \
    ///   cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib \
    ///     knowledge::embed::tests::embed_smoke -- --ignored --nocapture
    /// 验证：模型加载成功、向量维度合理、已归一化、相近语义余弦 > 无关语义。
    #[test]
    #[ignore]
    fn embed_smoke() {
        let emb = match Embedder::from_env_or_dir(None) {
            Ok(e) => e,
            Err(_) if std::env::var_os("PINVOU3_KB_EMBED_MODEL_DIR").is_none() => {
                eprintln!("SKIP: 未设 PINVOU3_KB_EMBED_MODEL_DIR");
                return;
            }
            Err(err) => panic!("embedding 模型加载失败: {err}"),
        };
        let v = emb.embed_one("汽车保险报价流程").expect("embed 失败");
        eprintln!("dim = {}", v.len());
        assert!(v.len() >= 256, "维度异常: {}", v.len());
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.02, "应已归一化, norm={norm}");

        let near = emb.embed_one("车险怎么报价").unwrap();
        let far = emb.embed_one("今天午饭吃什么").unwrap();
        let sim = cosine(&v, &near);
        let dis = cosine(&v, &far);
        eprintln!("相近={sim:.3} 无关={dis:.3}");
        assert!(sim > dis, "相近语义余弦应高于无关: {sim} vs {dis}");
    }
}
