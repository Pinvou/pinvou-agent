#!/bin/sh
# pinvou3 .deb 卸载前清理：删除超级权限 sudoers 文件。
# 否则用户不先在 UI 关 toggle 直接 apt remove pinvou3,会留下 NOPASSWD: ALL 授权 —— 安全黑洞。
# 失败不阻塞卸载(set -e 不开)。
rm -f /etc/sudoers.d/pinvou3 2>/dev/null || true

# 清理 postinst 放到各用户桌面的快捷方式(本地化目录名 + XDG 解析,与 postinst 对称)。
for uhome in /home/*; do
  [ -d "$uhome" ] || continue
  for c in "$uhome/桌面/pinvou3.desktop" "$uhome/Desktop/pinvou3.desktop"; do
    rm -f "$c" 2>/dev/null || true
  done
  if [ -r "$uhome/.config/user-dirs.dirs" ]; then
    ddir=$(. "$uhome/.config/user-dirs.dirs" 2>/dev/null; printf '%s' "$XDG_DESKTOP_DIR")
    [ -n "$ddir" ] && rm -f "$ddir/pinvou3.desktop" 2>/dev/null || true
  fi
done
exit 0
