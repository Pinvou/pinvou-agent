#!/bin/sh
# pinvou3 .deb 卸载前清理：删除超级权限 sudoers 文件。
# 否则用户不先在 UI 关 toggle 直接 apt remove pinvou3,会留下 NOPASSWD: ALL 授权 —— 安全黑洞。
# 失败不阻塞卸载(set -e 不开)。
rm -f /etc/sudoers.d/pinvou3 2>/dev/null || true
exit 0
