// 粘贴图片附件的公共前置：剪贴板事件里筛出图片文件 + FileReader 读为字节。
// 此前 ChatView（bridge addPasteImage）与 CodexAcpView（save_paste_image / 设备直传）
// 各自内联同一段 WebKit 兼容筛选与读取，jpeg→jpg 扩展名归一只在 codex 侧做了，
// 现统一收敛（聊天侧粘贴 image/jpeg 的落盘名由 .jpeg 归一为 .jpg）。

/**
 * 从 paste 事件取出的图片 File 列表。不调用 preventDefault——是否吃掉事件
 * （以及无可用通道时放行）由调用方决定。
 * WebKit 的 DataTransferItemList 无 Symbol.iterator，for...of/spread 会抛
 * TypeError，必须 Array.from（Safari/WKWebView 全版本）。
 */
export function collectClipboardImages(event) {
  // eslint-disable-next-line unicorn/prefer-spread -- DataTransferItemList is not iterable on any Safari/WKWebView version
  const items = Array.from((event.clipboardData && event.clipboardData.items) || []);
  return items
    .filter((item) => item.type && item.type.startsWith('image/'))
    .map((item) => item.getAsFile())
    .filter(Boolean);
}

/**
 * FileReader 读为字节数组（Safari 14 无 Blob#arrayBuffer，粘贴桥接路径保持
 * FileReader 读法）并推导扩展名；jpeg 统一归一为 jpg。
 * @returns {Promise<{ bytes: number[], ext: string }>}
 */
export function readPasteImageAsBytes(file) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      resolve({
        bytes: [...new Uint8Array(reader.result)],
        ext: (file.type.split('/')[1] || 'png').replace('jpeg', 'jpg'),
      });
    };
    reader.onerror = () => reject(reader.error || new Error('read paste image failed'));
    reader.readAsArrayBuffer(file);
  });
}
