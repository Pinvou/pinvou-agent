# 知识库模型来源

Pinvou 的本地知识库与共享知识库使用同一份 BGE-M3 INT8 ONNX 模型。运行时直接从 Hugging Face 上游仓库下载固定的五文件清单，不通过 Pinvou 的 GitHub Release 分发模型。

## 固定来源

- 仓库：[`onnx-community/bge-m3-ONNX`](https://huggingface.co/onnx-community/bge-m3-ONNX)
- Revision：`25b9af8e87a38eb120cfe87125383677b9cd309e`
- 上游许可证：[MIT（固定 revision 模型卡）](https://huggingface.co/onnx-community/bge-m3-ONNX/blob/25b9af8e87a38eb120cfe87125383677b9cd309e/README.md)

| 本地文件 | 上游文件 | 字节数 | SHA-256 |
|---|---|---:|---|
| `model.onnx` | [`onnx/model_int8.onnx`](https://huggingface.co/onnx-community/bge-m3-ONNX/resolve/25b9af8e87a38eb120cfe87125383677b9cd309e/onnx/model_int8.onnx?download=true) | 568,479,395 | `2237f770aad5c71bbc1fc2d361a57f9a37400574cc9eff32626f0cdb49234730` |
| `tokenizer.json` | [`tokenizer.json`](https://huggingface.co/onnx-community/bge-m3-ONNX/resolve/25b9af8e87a38eb120cfe87125383677b9cd309e/tokenizer.json?download=true) | 17,082,799 | `249df0778f236f6ece390de0de746838ef25b9d6954b68c2ee71249e0a9d8fd4` |
| `config.json` | [`config.json`](https://huggingface.co/onnx-community/bge-m3-ONNX/resolve/25b9af8e87a38eb120cfe87125383677b9cd309e/config.json?download=true) | 658 | `70dae5884ced999af00244f776ac9eaa71538d68497d3d6a6091e0318cd32905` |
| `tokenizer_config.json` | [`tokenizer_config.json`](https://huggingface.co/onnx-community/bge-m3-ONNX/resolve/25b9af8e87a38eb120cfe87125383677b9cd309e/tokenizer_config.json?download=true) | 1,203 | `b87c8703482b0300d3da30e201519aa641f6a450f5eb5bf1e624afbf70c74d80` |
| `special_tokens_map.json` | [`special_tokens_map.json`](https://huggingface.co/onnx-community/bge-m3-ONNX/resolve/25b9af8e87a38eb120cfe87125383677b9cd309e/special_tokens_map.json?download=true) | 964 | `8c785abebea9ae3257b61681b4e6fd8365ceafde980c21970d001e834cf10835` |

下载器先把文件写入候选目录，逐文件检查大小和 SHA-256，并真实加载模型；全部成功后才原子替换正式模型目录。失败或取消不会覆盖当前可用模型。

## Hugging Face 兼容镜像

默认根地址为 `https://huggingface.co`。共享知识库服务可设置：

```bash
PINVOU_KNOWLEDGE_HF_BASE_URL=https://hf-mirror.example.com
```

桌面端可设置 `PINVOU3_KB_HF_BASE_URL`；未设置时回退到 `PINVOU_KNOWLEDGE_HF_BASE_URL`。变量值必须是 Hugging Face `resolve` API 兼容的服务根地址，例如 `https://hf-mirror.example.com`，不要包含仓库名、revision、文件路径或查询参数。镜像无法绕过固定 revision、文件大小和 SHA-256 校验。
