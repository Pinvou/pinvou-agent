//! 桌面端兼容门面。向量实现由独立 `pinvou-knowledge` 核心统一提供；
//! ONNX Runtime 动态库定位仍由桌面宿主在加载前完成。

pub use pinvou_knowledge::embedding::{blob_to_vec, cosine, vec_to_blob, Embedder};
