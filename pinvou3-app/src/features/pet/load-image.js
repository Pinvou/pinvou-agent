/**
 * 加载并确保位图真正解码完成后才 resolve——onload 只代表数据就位，大图
 * WebP 首绘前仍可能解码导致闪帧。设置页选择器与桌宠窗口共用本实现，
 * 避免两份拷贝语义漂移（一份带 decode() 兜底、一份没有）。
 */
export function loadImage(url) {
  const image = new Image();
  image.src = url;
  if (typeof image.decode === 'function') {
    return image.decode().then(() => url);
  }
  return new Promise((resolve, reject) => {
    image.onload = () => resolve(url);
    image.onerror = () => reject(new Error(`image failed to load: ${url}`));
  });
}
