//! 小型字节编码工具。
//!
//! 历史上散落在 sessions / remote_control / voice / knowledge / app commands 等处的
//! sha256 → 十六进制编码,sha2 0.10 时多用 `format!("{:x}", hasher.finalize())`
//! (依赖 GenericArray 的 LowerHex impl),其余是手写 `for` 循环配 `write!`/`push_str`,
//! 风格不一、每处重复一遍。sha2 0.11 的 `Output`(GenericArray)不实现 LowerHex,
//! 前一种写法编译不过。集中到本函数既消除重复,又便于将来统一换实现(如换更快的查表法)。

/// 把字节切片编码为小写十六进制字符串。
///
/// 用 `write!` 逐字节写进预分配 `String`(容量恰好 2× 字节长度),避免 `.map().collect()`
/// 每字节分配一个临时 `String`。与 `hex::encode` 等价但无需新增直接依赖。
pub(crate) fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    // write! 到 String 永不失败(fmt::Error 仅出现在写入失败时,String 无 IO 失败)。
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        // 空输入 → 空串(原 empty 测试)。
        assert_eq!(hex_lower(&[]), "");
        assert_eq!(hex_lower(&[0x00]), "00");
        assert_eq!(hex_lower(&[0x0f, 0x10, 0xff]), "0f10ff");
        // SHA-256("") = e3b0c442...(前 16 字节)
        assert_eq!(
            &hex_lower(&[
                0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f,
                0xb9, 0x24,
            ])[..],
            "e3b0c44298fc1c149afbf4c8996fb924"
        );
        // 完整 32 字节(64 hex 字符):SHA-256("abc")——覆盖全 digest 长度的截断/边界。
        assert_eq!(
            &hex_lower(&[
                0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
                0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
                0xf2, 0x00, 0x15, 0xad,
            ])[..],
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn output_length_and_lowercase() {
        // 每字节恰好两个十六进制字符(原 output_length_is_two_chars_per_byte;
        // String 只保证容量不小于预分配值,不能把"容量恰好相等"当跨平台契约)。
        assert_eq!(hex_lower(&[0xab; 32]).len(), 64);
        // 输出只含小写 hex 字符(原 lowercase_only)。
        let s = hex_lower(&[0xfa, 0xbc]);
        assert_eq!(s, "fabc");
        assert!(s
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
    }
}
