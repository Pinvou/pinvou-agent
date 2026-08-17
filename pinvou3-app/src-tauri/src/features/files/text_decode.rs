//! 纯文本摄入与字节内容嗅探 / 解码。
//!
//! 负责 `ingest` 中「text」分类（已知文本扩展名）与未知扩展名 / 无扩展名的
//! 内容嗅探兜底：NUL/控制字符判定为二进制、UTF-16(BOM 与无 BOM 强信号)
//! 与 GB18030/GBK 解码、私钥内容侧拦截。解码产物 + 警告交回 facade 的
//! [`IngestResult`]。
//!
//! [`IngestResult`]: super::IngestResult

use std::path::Path;

use super::{binary_placeholder, estimate_tokens, secret_placeholder, IngestResult};

/// 已知文本扩展名（txt/md/json/csv/...）的摄入：读字节 → 私钥内容兜底 → 统一解码。
pub(super) fn ingest_text(
    path: &Path,
    basename: String,
    path_str: String,
    byte_size: u64,
) -> IngestResult {
    // 先读字节:严格 UTF-8 直接用;非法 UTF-8(如 GBK 中文 .txt)lossy 还原并标 warning,
    // 而非返回空 markdown —— 中文用户常有非 UTF-8 文本文件,lossy 能保住大部分可读内容。
    // 读字节本身失败才回「读取失败」。
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            return IngestResult::warning(
                "text",
                &basename,
                path,
                byte_size,
                format!("读取失败(可能不是文本): {e}"),
            );
        }
    };
    // 内容侧安全兜底:已知文本扩展名也可能被塞了私钥(如把 id_rsa 改名 id_rsa.txt),
    // 这种情况按 secret 拒绝读取,不进 markdown。
    if pinvou_knowledge::looks_like_secret_material(&bytes) {
        return secret_placeholder(basename, path_str, byte_size);
    }
    let (content, warning) = decode_text_bytes(&bytes);
    let tokens = content.as_ref().map(|c| estimate_tokens(c)).unwrap_or(0);
    IngestResult {
        kind: "text".into(),
        basename,
        path: path_str,
        markdown: content,
        token_estimate: tokens,
        byte_size,
        warning,
    }
}

/// 未知扩展名 / 无扩展名文件的兜底:先嗅探,是文本就当文本读(让模型看到内容),
/// 真二进制才回 binary_placeholder。
///
/// 非 UTF-8 文本优先按 GB18030(兼容 GBK)解码；只有 GB18030 也不合法时才 lossy。
pub(super) fn sniff_text_or_binary(
    path: &Path,
    basename: String,
    path_str: String,
    byte_size: u64,
) -> IngestResult {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        // 读失败(权限/TOCTOU 等)如实上报,不要误报成「不支持的二进制类型」。
        Err(e) => {
            return IngestResult::warning(
                "binary",
                &basename,
                path,
                byte_size,
                format!("文件读取失败: {e}"),
            );
        }
    };
    // 内容侧安全兜底:无扩展名 / 改名的私钥(id_rsa、server-key 等)在此拦截。
    if pinvou_knowledge::looks_like_secret_material(&bytes) {
        return secret_placeholder(basename, path_str, byte_size);
    }
    // UTF-16 无 BOM 的 ASCII/混合文本含大量 NUL，本来会被二进制嗅探拒绝；先识别
    // 奇偶字节 NUL 分布的强信号，确认不是 UTF-16 后再执行二进制降级。
    if detect_utf16(&bytes).is_none() && looks_like_binary_sample(&bytes) {
        return binary_placeholder(basename, path_str, byte_size);
    }
    // 判定为文本:统一解码(UTF-8 / UTF-16 / GB18030 / 最终 lossy 兜底)。
    let (content, encoding_warning) = decode_text_bytes(&bytes);
    let tokens = content.as_ref().map(|c| estimate_tokens(c)).unwrap_or(0);
    IngestResult {
        kind: "text".into(),
        basename,
        path: path_str,
        markdown: content,
        token_estimate: tokens,
        byte_size,
        warning: encoding_warning,
    }
}

/// 对字节内容做嗅探,判定是文本还是二进制。只看前 `SNIFF_BYTES` 字节即可下结论
/// (NUL / 控制字符都在头部就能判定),**但调用方仍会 `std::fs::read` 整个文件** ——
/// 因为判定为文本时还需要全部字节做解码。`ingest` 入口已用 MAX_FILE_BYTES(20MiB)
/// 挡住超大文件,故整文件读取的内存上界就是 20MiB,安全。
///
/// 算法(业界标准,`file` / `grep -I` / vim 同源):
/// 1. 含 NUL 字节(0x00)→ 二进制(确定性最强,文本文件几乎不会含 NUL);
/// 2. 控制字符(除 `\t \n \r \f`)占比 > 30% → 二进制;
/// 3. 否则 → 文本。
pub(super) fn looks_like_binary_sample(bytes: &[u8]) -> bool {
    const SNIFF_BYTES: usize = 8192;
    let sample = if bytes.len() > SNIFF_BYTES {
        &bytes[..SNIFF_BYTES]
    } else {
        bytes
    };
    if sample.is_empty() {
        return false; // 空文件不当二进制(此处的 sample.is_empty() 判定兜底)
    }
    // NUL 字节 → 二进制(强信号)。
    if sample.contains(&0u8) {
        return true;
    }
    // 控制字符占比:除 \t(0x09) \n(0x0A) \r(0x0D) \f(0x0C) 外的 0x01..=0x1F。
    // > 30% 视为二进制(纯文本通常 < 5%,二进制常 > 50%)。
    let ctrl = sample
        .iter()
        .filter(|&&b| b < 0x20 && !matches!(b, 0x09 | 0x0A | 0x0D | 0x0C))
        .count();
    ctrl * 10 > sample.len() * 3 // ctrl/sample > 0.3,用整数乘法避浮点
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Utf16Endian {
    Little,
    Big,
}

/// 识别 UTF-16 编码并返回 `(字节序, BOM 长度)`。
///
/// 有 BOM 时是确定性判断；无 BOM 时只接受强信号：至少两个 code unit 的同一侧为 NUL、
/// 覆盖至少一半采样对，且另一侧 NUL 数不到其四分之一。这能接住常见 ASCII/中文混合
/// UTF-16，同时避免仅凭单个 NUL 把普通二进制误判成文本。
fn detect_utf16(bytes: &[u8]) -> Option<(Utf16Endian, usize)> {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return Some((Utf16Endian::Little, 2));
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return Some((Utf16Endian::Big, 2));
    }
    if bytes.len() < 4 || !bytes.len().is_multiple_of(2) {
        return None;
    }

    let sample_len = bytes.len().min(8192) & !1;
    let sample = &bytes[..sample_len];
    let pairs = sample.len() / 2;
    let even_nuls = sample.iter().step_by(2).filter(|&&b| b == 0).count();
    let odd_nuls = sample
        .iter()
        .skip(1)
        .step_by(2)
        .filter(|&&b| b == 0)
        .count();
    let strong = |dominant: usize, other: usize| {
        dominant >= 2 && dominant * 2 >= pairs && dominant > other.saturating_mul(4)
    };

    if strong(odd_nuls, even_nuls) {
        Some((Utf16Endian::Little, 0))
    } else if strong(even_nuls, odd_nuls) {
        Some((Utf16Endian::Big, 0))
    } else {
        None
    }
}

fn decode_utf16(
    bytes: &[u8],
    endian: Utf16Endian,
    bom_len: usize,
) -> (Option<String>, Option<String>) {
    let payload = &bytes[bom_len..];
    let label = match endian {
        Utf16Endian::Little => "UTF-16 LE",
        Utf16Endian::Big => "UTF-16 BE",
    };
    if !payload.len().is_multiple_of(2) {
        return (
            None,
            Some(format!("检测到 {label} 编码,但文件字节数不完整,未读取内容")),
        );
    }
    let units: Vec<u16> = payload
        .chunks_exact(2)
        .map(|pair| match endian {
            Utf16Endian::Little => u16::from_le_bytes([pair[0], pair[1]]),
            Utf16Endian::Big => u16::from_be_bytes([pair[0], pair[1]]),
        })
        .collect();
    match String::from_utf16(&units) {
        Ok(text) => {
            let bom = if bom_len == 0 { "无 BOM " } else { "" };
            (
                Some(text.trim_start_matches('\u{feff}').to_string()),
                Some(format!("检测到 {bom}{label} 编码,已转换为 UTF-8")),
            )
        }
        Err(_) => (
            None,
            Some(format!("检测到 {label} 编码,但内容损坏,未读取内容")),
        ),
    }
}

/// 把已读到的字节统一解码成文本内容,供 `ingest_text` 与 `sniff_text_or_binary` 复用。
/// 返回 `(内容, 警告)`:
/// - **UTF-16 LE / BE**:BOM 确定识别；无 BOM 仅按强 NUL 分布信号识别并转 UTF-8；
/// - **UTF-8 BOM**(`EF BB BF` → U+FEFF):去掉开头 BOM,与 `ingest_office_text`
///   的 `s.trim_start_matches('\u{feff}')` 处理一致,避免污染正文开头。
/// - **严格 UTF-8**:直接用,无 warning。
/// - **GB18030 / GBK**:严格解码成 Unicode,标 warning 说明发生了编码转换；
/// - **其他非 UTF-8**:最后才 `from_utf8_lossy`,并明确提示可能有替换符。
pub(super) fn decode_text_bytes(bytes: &[u8]) -> (Option<String>, Option<String>) {
    if let Some((endian, bom_len)) = detect_utf16(bytes) {
        return decode_utf16(bytes, endian, bom_len);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => {
            let s = s.strip_prefix('\u{feff}').unwrap_or(s);
            (Some(s.to_string()), None)
        }
        Err(_) => {
            if let Some(decoded) =
                encoding_rs::GB18030.decode_without_bom_handling_and_without_replacement(bytes)
            {
                return (
                    Some(decoded.into_owned()),
                    Some("文件非 UTF-8 编码,已按 GB18030/GBK 转换为 UTF-8".into()),
                );
            }
            let lossy = String::from_utf8_lossy(bytes);
            let s = lossy.strip_prefix('\u{feff}').unwrap_or(&lossy);
            (
                Some(s.to_string()),
                Some("文件非 UTF-8 编码,已尽力还原(部分字符可能为替换符)".into()),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_like_binary_detects_nul_and_control_chars() {
        // NUL 字节 → 二进制(强信号)。
        assert!(looks_like_binary_sample(b"abc\x00def"));
        // 纯文本(含 UTF-8 多字节中文)→ 非二进制。
        assert!(!looks_like_binary_sample(
            "hello world\n中文测试".as_bytes()
        ));
        // 大量控制字符 → 二进制。
        let mut ctrl = vec![0x01u8; 1000];
        ctrl.extend_from_slice(b"text");
        assert!(looks_like_binary_sample(&ctrl));
        // 正常含 \t \n \r → 非二进制。
        assert!(!looks_like_binary_sample(b"col1\tcol2\nrow1\r\nrow2"));
        // 空文件 → 非二进制(上层 byte_size==0 已挡)。
        assert!(!looks_like_binary_sample(b""));
    }

    #[test]
    fn looks_like_binary_30_percent_boundary() {
        // 控制字符占比阈值:严格 > 30% 才判二进制(ctrl*10 > len*3)。
        // 30/100 = 30% → 非二进制(边界外);31/100 > 30% → 二进制。
        let text_byte = b'x';
        let ctrl_byte = 0x01u8;
        let make = |ctrl: usize, total: usize| {
            let mut v = vec![text_byte; total];
            for b in v.iter_mut().take(ctrl) {
                *b = ctrl_byte;
            }
            v
        };
        assert!(
            !looks_like_binary_sample(&make(30, 100)),
            "恰好 30% 控制字符应为文本(严格大于才判二进制)"
        );
        assert!(
            looks_like_binary_sample(&make(31, 100)),
            "31% 控制字符应为二进制"
        );
        // 8192 采样边界:8192 全采,8193 截断到 8192 —— 只要前 8192 判定一致即可。
        let mut big_text = vec![text_byte; 8193];
        big_text[0] = 0x00; // 头部一个 NUL → 二进制(采样必命中)
        assert!(looks_like_binary_sample(&big_text));
    }
}
