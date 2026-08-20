#!/bin/sh
# Refresh global desktop caches and start the fixed socket for sessions that are online now.
# The package never sources files from a user's home, writes desktop shortcuts into a home,
# or creates a permanent global/user enable symlink. The desktop helper can socket-activate
# the service after the next login.

[ -x /usr/bin/update-desktop-database ] && \
  /usr/bin/update-desktop-database -q /usr/share/applications 2>/dev/null || true
[ -x /usr/bin/gtk-update-icon-cache ] && \
  /usr/bin/gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor 2>/dev/null || true

online_user_command() {
  operation="$1"
  [ -x /usr/sbin/runuser ] && runuser_bin=/usr/sbin/runuser || runuser_bin=/usr/bin/runuser
  [ -x /usr/bin/getent ] && getent_bin=/usr/bin/getent || getent_bin=/bin/getent
  [ -x /usr/bin/systemctl ] && systemctl_bin=/usr/bin/systemctl || systemctl_bin=/bin/systemctl
  [ -x /usr/bin/stat ] && stat_bin=/usr/bin/stat || stat_bin=/bin/stat
  [ -x /usr/bin/env ] && env_bin=/usr/bin/env || env_bin=/bin/env
  [ -x "$runuser_bin" ] && [ -x "$getent_bin" ] && [ -x "$systemctl_bin" ] \
    && [ -x "$stat_bin" ] && [ -x "$env_bin" ] || return 0

  for runtime_dir in /run/user/*; do
    [ -d "$runtime_dir" ] || continue
    uid=${runtime_dir##*/}
    case "$uid" in ''|*[!0-9]*|0) continue ;; esac
    [ "$($stat_bin -c %u "$runtime_dir" 2>/dev/null)" = "$uid" ] || continue
    [ -S "$runtime_dir/bus" ] || continue
    passwd_record=$($getent_bin passwd "$uid" 2>/dev/null) || continue
    user_name=${passwd_record%%:*}
    passwd_tail=${passwd_record#*:}
    passwd_tail=${passwd_tail#*:}
    passwd_uid=${passwd_tail%%:*}
    [ "$passwd_uid" = "$uid" ] || continue
    case "$user_name" in ''|*[!A-Za-z0-9_.-]*) continue ;; esac

    "$runuser_bin" --user "$user_name" -- "$env_bin" \
      XDG_RUNTIME_DIR="$runtime_dir" \
      DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime_dir/bus" \
      "$systemctl_bin" --user daemon-reload 2>/dev/null || true
    "$runuser_bin" --user "$user_name" -- "$env_bin" \
      XDG_RUNTIME_DIR="$runtime_dir" \
      DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime_dir/bus" \
      "$systemctl_bin" --user "$operation" pinvou3-supervisor.socket 2>/dev/null || true
  done
}

online_user_command start
exit 0
