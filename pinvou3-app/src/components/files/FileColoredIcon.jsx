import { resolveAppAssetUrl } from '../../shared/asset-url.mjs';
import { resolveFileIcon } from './file-icon-theme.js';

// VSCode 风格彩色文件图标：按文件名/目录状态从 Material Icon Theme 子集取图。
export function FileColoredIcon({ name, isDir = false, isOpen = false, size = 14, className = '' }) {
  const iconFile = resolveFileIcon(name, { isDir, isOpen });
  return (
    <img
      src={resolveAppAssetUrl(`file-icons/theme/${iconFile}`)}
      alt=""
      width={size}
      height={size}
      draggable={false}
      className={`shrink-0 object-contain${className ? ' ' + className : ''}`}
    />
  );
}
