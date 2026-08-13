// 技能包(zip)导入的纯逻辑:拖放文件里挑 zip、读字节转 base64、大小软限。
// 与后端 `import_skill_package_bytes`(Rust 强校验)配套,这里只做展示层友好处理。

/// zip 未压缩大小软限,对齐 Rust `MAX_SKILL_SIZE_BYTES`(5 MiB)。
/// 前端超限直接提示,避免把超限字节传给后端(后端仍会强校验)。
export const MAX_SKILL_ZIP_BYTES = 5 * 1024 * 1024;

/// 从拖放文件列表中挑第一个 .zip(大小写不敏感);没有则 null。
export function pickSkillZip(files) {
  const list = files || [];
  for (let i = 0; i < list.length; i++) {
    const f = list[i];
    if (f && typeof f.name === 'string' && /\.zip$/i.test(f.name)) return f;
  }
  return null;
}

/// 读 File 为 base64 字符串(0x8000 分块拼接,与 platform/tauri/bridge/artifacts.js
/// encodeBase64Bytes 同构,避免超大 String.fromCharCode.apply 栈溢出)。
export function fileToBase64(file) {
  return file.arrayBuffer().then((buf) => {
    const bytes = new Uint8Array(buf);
    let binary = '';
    const stride = 0x8000;
    for (let offset = 0; offset < bytes.length; offset += stride) {
      binary += String.fromCharCode.apply(null, bytes.subarray(offset, offset + stride));
    }
    return btoa(binary);
  });
}
