// 插件包(zip)导入的纯逻辑:拖放文件里挑 zip/md、读字节转 base64、大小软限。
// 与后端 `import_plugin_package_bytes_cmd`(Rust 强校验)配套,这里只做展示层友好处理。

/// zip 未压缩大小软限。拖放走 base64 字节通道,过大有内存开销,这里给 50 MiB 的
/// 前端软限(仅拖放路径;原生对话框按钮路径不受此限)。后端 `MAX_PLUGIN_SIZE_BYTES`
/// 仍以 200 MiB 强校验。
export const MAX_SKILL_ZIP_BYTES = 50 * 1024 * 1024;

/// 从拖放文件列表中挑第一个可导入的技能文件：`.zip`(插件包) 或 `.md`/`.markdown`
/// (单个技能文件)。返回 { file, kind: 'zip' | 'md' }；没有则 null。
export function pickSkillDrop(files) {
  const list = files || [];
  for (let i = 0; i < list.length; i++) {
    const f = list[i];
    if (!f || typeof f.name !== 'string') continue;
    if (/\.zip$/i.test(f.name)) return { file: f, kind: 'zip' };
    if (/\.(md|markdown)$/i.test(f.name)) return { file: f, kind: 'md' };
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
