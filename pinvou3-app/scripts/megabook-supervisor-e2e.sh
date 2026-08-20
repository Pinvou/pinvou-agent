#!/bin/sh
# MegaBook install-level acceptance harness for the fixed PinvouOS Supervisor profile.
#
# Workflow (no command here invokes sudo):
#   1. baseline /home/<user>/pinvou3_<version>_<revision>_amd64.deb
#   2. user installs that exact deb with sudo apt-get --no-install-recommends
#   3. verify-safe, or the explicit and disruptive verify-memory-max mode
#   4. prepare-purge
#   5. user purges pinvou3 with sudo
#
# Installed attestation proves the selected data payload, every md5sums-listed file, generated
# package path list, maintainer control members, and install-behavior control fields match the
# baseline deb. It does not reconstruct or claim equality of the original archive compression.
#
# verify-memory-max installs a hash-pinned, runtime-only ExecStartPost fixture. The fixture forks a
# one-shot child inside pinvou3-app.service, first crosses MemoryHigh to exercise the real ASR
# Governor path, then crosses MemoryMax in a fresh app generation. The app group is expected to be
# killed; the Supervisor must remain outside that cgroup and retain exact evidence.

set -eu
umask 077

APP_UNIT=pinvou3-app.service
ASR_UNIT=pinvou-qwen3-asr.service
SUPERVISOR_UNIT=pinvou3-supervisor.service
SOCKET_UNIT=pinvou3-supervisor.socket
PROFILE_HELPER=/usr/lib/pinvou3/supervisor/pinvou-megabook-profile
SUPERVISOR=/usr/lib/pinvou3/supervisor/pinvou-supervisor
GENERIC_DESKTOP=/usr/share/applications/pinvou3.desktop
PROFILE_SHA256=74cc705379e10f6626bb614118e66c080366e3bed907509a786d7692048e451c
DESKTOP_SHA256=ddfe6a25920570d8992a9eb6c3d53bcc64404a6ce069e764e42383141e9a12a0
LOADER_SHA256=e740b1c6632b2cdd10158fed72c4760720c80956e6c93ba5ae19c929b9800cde
HIGH_DROPIN_SHA256=13c32ca901b5e45411fcf597b21373817b3d3893c8fc875035a5928c1dd35d47
MAX_DROPIN_SHA256=fd6a23395c235e5344e8fc9d346403c0413738832afa2e34bc41a86f0e541e08
HIGH_GO_SHA256=6319b41e829ccc8fd69446d15ecbae665ef60942acd56458ea690abd3d5e8c30
MAX_GO_SHA256=9fbddcd505d602fe6292020d95b9618486cacfb304f5ba30729ac0794d91da63
HIGH_ONCE_SHA256=6de6d9ddb84044e659912be3e2ee6ad7c640620875966758c2c0157bfbcf407d
MAX_ONCE_SHA256=ef53463024ffdf83e9b1c054b817f70881f83d0121a64af14d9f9e522f6adbdc

profile_owned=0
app_started=0
socket_was_active=0
supervisor_was_active=0
asr_was_active=0
cleanup_enabled=0
baseline_tmp=
purge_tmp=
fixture_tmp=

fail() {
  /usr/bin/printf '%s\n' "megabook-supervisor-e2e: $*" >&2
  exit 1
}

for required_tool in /usr/bin/awk /usr/bin/cmp /usr/bin/dirname /usr/bin/dpkg /usr/bin/dpkg-deb \
  /usr/bin/dpkg-query /usr/bin/getent /usr/bin/grep /usr/bin/id /usr/bin/install \
  /usr/bin/journalctl \
  /usr/bin/ln /usr/bin/mkdir /usr/bin/mktemp /usr/bin/printf \
  /usr/bin/python3 /usr/bin/rm /usr/bin/rmdir /usr/bin/sha256sum /usr/bin/sleep \
  /usr/bin/stat /usr/bin/systemctl; do
  [ -x "$required_tool" ] || fail "required fixed tool is unavailable: $required_tool"
done

script_dir=$(CDPATH= cd -- "$(/usr/bin/dirname -- "$0")" && pwd -P) \
  || fail "cannot resolve the harness directory"
fixture_dir=$script_dir/fixtures/megabook-supervisor-e2e
loader_source=$fixture_dir/memory-loader.py
high_dropin_source=$fixture_dir/90-memory-high.conf
max_dropin_source=$fixture_dir/90-memory-max.conf
high_go_source=$fixture_dir/go-high.marker
max_go_source=$fixture_dir/go-max.marker

uid=$(/usr/bin/id -u) || fail "cannot determine the effective uid"
case "$uid" in
  ''|*[!0-9]*|0) fail "must run as a non-root login user" ;;
esac
passwd_record=$(/usr/bin/getent passwd "$uid") || fail "cannot resolve the effective uid"
case "$passwd_record" in
  *"
"*) fail "passwd lookup returned more than one record" ;;
esac
passwd_tail=${passwd_record#*:}
passwd_tail=${passwd_tail#*:}
passwd_uid=${passwd_tail%%:*}
passwd_tail=${passwd_tail#*:}
passwd_tail=${passwd_tail#*:}
passwd_tail=${passwd_tail#*:}
home_dir=${passwd_tail%%:*}
[ "$passwd_uid" = "$uid" ] || fail "passwd uid does not match the effective uid"
case "$home_dir" in
  /|''|*'//'*) fail "login home is not a bounded absolute path" ;;
  /*) ;;
  *) fail "login home is not absolute" ;;
esac
case "/${home_dir#/}/" in
  *'/../'*|*'/./'*) fail "login home contains an unsafe path component" ;;
esac

e2e_state_dir=$home_dir/.local/state/pinvou-megabook-e2e
baseline_file=$e2e_state_dir/baseline-v1
purge_file=$e2e_state_dir/purge-baseline-v1
profile_target=$home_dir/.config/systemd/user/pinvou3-app.service.d/50-megabook-canary.conf
desktop_target=$home_dir/.local/share/applications/pinvou3-megabook-canary.desktop
profile_state_dir=$home_dir/.local/state/pinvou3
profile_staging_dir=${profile_target%/*}/.pinvou-profile-staging-v2
desktop_staging_dir=${desktop_target%/*}/.pinvou-desktop-staging-v2
marker_staging_dir=$profile_state_dir/.pinvou-marker-staging-v2
legacy_profile_marker=$profile_state_dir/megabook-profile-v1.registered
installing_profile_marker=$profile_state_dir/megabook-profile-v2.installing
applied_profile_marker=$profile_state_dir/megabook-profile-v2.applied
profile_quarantine=$home_dir/.config/systemd/user/pinvou3-app.service.d/.pinvou-quarantine-profile-v2
desktop_quarantine=$home_dir/.local/share/applications/.pinvou-quarantine-desktop-v2
legacy_marker_quarantine=$profile_state_dir/.pinvou-quarantine-marker-v1
installing_marker_quarantine=$profile_state_dir/.pinvou-quarantine-marker-v2-installing
applied_marker_quarantine=$profile_state_dir/.pinvou-quarantine-marker-v2-applied
supervisor_state_dir=$home_dir/.local/state/pinvou-supervisor
control_ledger=$supervisor_state_dir/control-v1.jsonl
observation_journal=$supervisor_state_dir/observations-v1.jsonl
runtime_ledger=$home_dir/.pinvou3/pinvou-os/events.v1.jsonl
runtime_dir=/run/user/$uid
e2e_runtime_dir=$runtime_dir/pinvou-megabook-e2e
loader_target=$e2e_runtime_dir/memory-loader.py
high_once_marker=$e2e_runtime_dir/once-high.marker
max_once_marker=$e2e_runtime_dir/once-max.marker
high_evidence=$e2e_runtime_dir/loader-high.json
max_evidence=$e2e_runtime_dir/loader-max.json
high_go_marker=$e2e_runtime_dir/go-high.marker
max_go_marker=$e2e_runtime_dir/go-max.marker
runtime_unit_dir=$runtime_dir/systemd/user
e2e_dropin_dir=$runtime_unit_dir/pinvou3-app.service.d
e2e_dropin_target=$e2e_dropin_dir/90-megabook-e2e-memory.conf

is_active() {
  [ "$(/usr/bin/systemctl --user show "$1" --property=ActiveState --value 2>/dev/null)" = active ]
}

fixed_stop_app() {
  /usr/bin/systemctl --user stop "$APP_UNIT" >/dev/null 2>&1 || return 1
  [ "$(/usr/bin/systemctl --user show "$APP_UNIT" --property=MainPID --value)" = 0 ]
}

cleanup_run() {
  saved_status=$?
  trap - EXIT HUP INT TERM
  cleanup_failed=0
  set +e
  baseline_tmp=
  purge_tmp=
  fixture_tmp=
  recover_all_staging_orphans || cleanup_failed=1
  if [ "$cleanup_enabled" -eq 0 ]; then
    [ "$cleanup_failed" -eq 0 ] || saved_status=1
    exit "$saved_status"
  fi
  if [ "$app_started" -eq 1 ]; then
    fixed_stop_app || cleanup_failed=1
  fi
  remove_e2e_assets || cleanup_failed=1
  /usr/bin/systemctl --user reset-failed "$APP_UNIT" >/dev/null 2>&1 || cleanup_failed=1
  if [ "$profile_owned" -eq 1 ]; then
    "$PROFILE_HELPER" deactivate >/dev/null 2>&1 || cleanup_failed=1
  fi
  if [ "$asr_was_active" -eq 1 ] && ! is_active "$ASR_UNIT"; then
    /usr/bin/systemctl --user start "$ASR_UNIT" >/dev/null 2>&1 || cleanup_failed=1
  fi
  if [ "$supervisor_was_active" -eq 0 ]; then
    /usr/bin/systemctl --user stop "$SUPERVISOR_UNIT" >/dev/null 2>&1 || cleanup_failed=1
  fi
  if [ "$socket_was_active" -eq 0 ]; then
    /usr/bin/systemctl --user stop "$SOCKET_UNIT" >/dev/null 2>&1 || cleanup_failed=1
  else
    /usr/bin/systemctl --user start "$SOCKET_UNIT" >/dev/null 2>&1 || cleanup_failed=1
  fi
  if [ "$supervisor_was_active" -eq 1 ]; then
    /usr/bin/systemctl --user start "$SUPERVISOR_UNIT" >/dev/null 2>&1 || cleanup_failed=1
  fi
  assert_captured_unit_state "$SOCKET_UNIT" "$socket_was_active" socket \
    || cleanup_failed=1
  assert_captured_unit_state "$SUPERVISOR_UNIT" "$supervisor_was_active" service \
    || cleanup_failed=1
  assert_captured_unit_state "$APP_UNIT" 0 service || cleanup_failed=1
  if [ "$asr_was_active" -eq 1 ]; then
    assert_captured_unit_state "$ASR_UNIT" 1 service || cleanup_failed=1
  fi
  for rollback_target in \
    "$e2e_dropin_target" "$loader_target" \
    "$high_once_marker" "$max_once_marker" \
    "$high_go_marker" "$max_go_marker" \
    "$high_evidence" "$max_evidence" \
    "$profile_target" "$desktop_target" \
    "$profile_staging_dir" "$desktop_staging_dir" "$marker_staging_dir" \
    "$legacy_profile_marker" "$installing_profile_marker" "$applied_profile_marker" \
    "$profile_quarantine" "$desktop_quarantine" \
    "$legacy_marker_quarantine" "$installing_marker_quarantine" \
    "$applied_marker_quarantine"; do
    if [ -e "$rollback_target" ] || [ -L "$rollback_target" ]; then
      cleanup_failed=1
    fi
  done
  if [ -e "$e2e_runtime_dir" ] || [ -L "$e2e_runtime_dir" ]; then
    cleanup_failed=1
  fi
  transaction_residue_absent || cleanup_failed=1
  set -e
  if [ "$cleanup_failed" -ne 0 ]; then
    /usr/bin/printf '%s\n' \
      'megabook-supervisor-e2e: rollback was incomplete; do not purge the package' >&2
    exit 1
  fi
  exit "$saved_status"
}

trap cleanup_run EXIT
trap 'exit 1' HUP INT TERM

validate_owned_directory() {
  directory=$1
  [ ! -L "$directory" ] || fail "directory must not be a symlink: $directory"
  [ -d "$directory" ] || fail "directory is missing: $directory"
  [ "$(/usr/bin/stat -c %u "$directory")" = "$uid" ] \
    || fail "directory owner mismatch: $directory"
  permissions=$(/usr/bin/stat -c %A "$directory")
  case "$permissions" in
    ?????w*|????????w*) fail "directory is group/other writable: $directory" ;;
  esac
}

ensure_state_directory() {
  for directory in "$home_dir/.local" "$home_dir/.local/state" "$e2e_state_dir"; do
    if [ ! -e "$directory" ]; then
      /usr/bin/mkdir -m 0700 -- "$directory" || fail "cannot create state directory"
    fi
    validate_owned_directory "$directory"
  done
}

validate_user_file() {
  validate_user_file_links "$1" "$2" 1
}

validate_user_file_links() {
  file=$1
  expected_mode=$2
  expected_links=$3
  [ ! -L "$file" ] || fail "file must not be a symlink: $file"
  [ -f "$file" ] || fail "file is missing: $file"
  [ "$(/usr/bin/stat -c %u:%a:%h "$file")" = "$uid:$expected_mode:$expected_links" ] \
    || fail "file owner/mode/link-count mismatch: $file"
}

fsync_path() {
  kind=$1
  path=$2
  /usr/bin/python3 -I - "$kind" "$path" <<'PY'
import os, stat, sys

kind, path = sys.argv[1:]
flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
if kind == "directory":
    flags |= getattr(os, "O_DIRECTORY", 0)
fd = os.open(path, flags)
try:
    mode = os.fstat(fd).st_mode
    if kind == "file" and not stat.S_ISREG(mode):
        raise SystemExit("fsync target is not a regular file")
    if kind == "directory" and not stat.S_ISDIR(mode):
        raise SystemExit("fsync target is not a directory")
    os.fsync(fd)
finally:
    os.close(fd)
PY
}

fsync_file() {
  fsync_path file "$1" || fail "cannot fsync private file: $1"
}

fsync_directory() {
  fsync_path directory "$1" || fail "cannot fsync private directory: $1"
}

# Recover only mktemp names from the harness' fixed private namespaces. A one-link file is an
# unpublished/partially populated staging file. A two-link file is removed only when its peer is
# one of the fixed publication targets and both names still identify the same inode. Anything else
# is preserved so an operator can inspect the concrete path instead of the harness guessing.
recover_staging_namespace() {
  directory=$1
  prefix=$2
  allowed_modes=$3
  shift 3
  /usr/bin/python3 -I - "$directory" "$prefix" "$uid" "$allowed_modes" "$@" <<'PY'
import hashlib
import os
import re
import stat
import sys

directory, prefix, uid_text, allowed_text, *target_fields = sys.argv[1:]
uid = int(uid_text)
allowed_modes = {int(value, 8) for value in allowed_text.split(",")}
if len(target_fields) % 3:
    raise SystemExit("fixed staging recovery target specification is malformed")
targets = [(target_fields[index], int(target_fields[index + 1], 8), target_fields[index + 2])
           for index in range(0, len(target_fields), 3)]

try:
    directory_lstat = os.lstat(directory)
except FileNotFoundError:
    raise SystemExit(0)
if stat.S_ISLNK(directory_lstat.st_mode) or not stat.S_ISDIR(directory_lstat.st_mode):
    raise SystemExit(f"staging namespace is not a real directory: {directory}")
if directory_lstat.st_uid != uid or stat.S_IMODE(directory_lstat.st_mode) != 0o700:
    raise SystemExit(f"staging namespace owner/mode mismatch: {directory}")

directory_flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) \
    | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0)
file_flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0) \
    | getattr(os, "O_NONBLOCK", 0)
directory_fd = os.open(directory, directory_flags)
changed = False
try:
    opened_directory = os.fstat(directory_fd)
    if (opened_directory.st_dev, opened_directory.st_ino) != (
        directory_lstat.st_dev, directory_lstat.st_ino
    ):
        raise SystemExit(f"staging namespace changed while opening it: {directory}")
    for name in sorted(os.listdir(directory_fd)):
        if not name.startswith(prefix):
            continue
        path = os.path.join(directory, name)
        suffix = name[len(prefix):]
        if re.fullmatch(r"[A-Za-z0-9]{6}", suffix) is None:
            raise SystemExit(f"reserved staging name has an invalid suffix; preserved: {path}")
        before = os.stat(name, dir_fd=directory_fd, follow_symlinks=False)
        if stat.S_ISLNK(before.st_mode) or not stat.S_ISREG(before.st_mode):
            raise SystemExit(f"reserved staging path is not a regular non-symlink; preserved: {path}")
        mode = stat.S_IMODE(before.st_mode)
        if before.st_uid != uid or mode not in allowed_modes:
            raise SystemExit(f"reserved staging owner/mode mismatch; preserved: {path}")
        if before.st_nlink not in (1, 2):
            raise SystemExit(f"reserved staging link count is not recoverable; preserved: {path}")

        candidate_fd = os.open(name, file_flags, dir_fd=directory_fd)
        try:
            opened = os.fstat(candidate_fd)
            if (opened.st_dev, opened.st_ino, opened.st_nlink) != (
                before.st_dev, before.st_ino, before.st_nlink
            ) or not stat.S_ISREG(opened.st_mode) or opened.st_uid != uid \
                    or stat.S_IMODE(opened.st_mode) != mode:
                raise SystemExit(f"reserved staging path changed while opening it; preserved: {path}")

            if before.st_nlink == 2:
                peer_found = False
                candidate_hash = None
                for target, expected_mode, expected_hash in targets:
                    try:
                        target_lstat = os.lstat(target)
                    except FileNotFoundError:
                        continue
                    if stat.S_ISLNK(target_lstat.st_mode) or not stat.S_ISREG(target_lstat.st_mode):
                        continue
                    if target_lstat.st_uid != uid \
                            or stat.S_IMODE(target_lstat.st_mode) != expected_mode \
                            or target_lstat.st_nlink != 2:
                        continue
                    if (target_lstat.st_dev, target_lstat.st_ino) != (before.st_dev, before.st_ino):
                        continue
                    if expected_hash != "-":
                        if re.fullmatch(r"[0-9a-f]{64}", expected_hash) is None:
                            raise SystemExit("fixed staging recovery hash specification is malformed")
                        if candidate_hash is None:
                            digest = hashlib.sha256()
                            offset = 0
                            while True:
                                block = os.pread(candidate_fd, 1024 * 1024, offset)
                                if not block:
                                    break
                                digest.update(block)
                                offset += len(block)
                            candidate_hash = digest.hexdigest()
                        if candidate_hash != expected_hash:
                            continue
                    peer_found = True
                    break
                if not peer_found:
                    raise SystemExit(
                        f"two-link staging path has no fixed same-inode target; preserved: {path}"
                    )

            current = os.stat(name, dir_fd=directory_fd, follow_symlinks=False)
            if (current.st_dev, current.st_ino, current.st_nlink) != (
                before.st_dev, before.st_ino, before.st_nlink
            ) or not stat.S_ISREG(current.st_mode) or current.st_uid != uid \
                    or stat.S_IMODE(current.st_mode) != mode:
                raise SystemExit(f"reserved staging path changed before unlink; preserved: {path}")
            os.unlink(name, dir_fd=directory_fd)
            changed = True
        finally:
            os.close(candidate_fd)
    if changed:
        os.fsync(directory_fd)
finally:
    os.close(directory_fd)
PY
}

recover_state_staging_orphans() {
  recover_staging_namespace \
    "$e2e_state_dir" .baseline. 600 "$baseline_file" 600 - || return 1
  recover_staging_namespace \
    "$e2e_state_dir" .purge. 600 "$purge_file" 600 - || return 1
}

recover_e2e_staging_orphans() {
  recover_staging_namespace \
    "$e2e_runtime_dir" .pinvou-e2e. 600,700 \
    "$loader_target" 700 "$LOADER_SHA256" \
    "$high_go_marker" 600 "$HIGH_GO_SHA256" \
    "$max_go_marker" 600 "$MAX_GO_SHA256" || return 1
  recover_staging_namespace \
    "$e2e_dropin_dir" .pinvou-e2e. 600,644 \
    "$e2e_dropin_target" 644 "$HIGH_DROPIN_SHA256" \
    "$e2e_dropin_target" 644 "$MAX_DROPIN_SHA256" || return 1
  if [ -d "$e2e_dropin_dir" ] && [ ! -L "$e2e_dropin_dir" ]; then
    /usr/bin/rmdir -- "$e2e_dropin_dir" >/dev/null 2>&1 || true
  fi
  if [ -d "$e2e_runtime_dir" ] && [ ! -L "$e2e_runtime_dir" ]; then
    /usr/bin/rmdir -- "$e2e_runtime_dir" >/dev/null 2>&1 || true
  fi
}

recover_all_staging_orphans() {
  recover_state_staging_orphans || return 1
  recover_e2e_staging_orphans || return 1
  return 0
}

publish_private_staged_file() {
  staged=$1
  target=$2
  directory=$3
  validate_user_file "$staged" 600
  fsync_file "$staged"
  staged_identity=$(/usr/bin/stat -c %d:%i "$staged") \
    || fail "cannot identify private staging file"
  /usr/bin/ln -T -- "$staged" "$target" \
    || fail "private target appeared concurrently; refusing to overwrite it: $target"
  fsync_directory "$directory"
  validate_user_file_links "$staged" 600 2
  validate_user_file_links "$target" 600 2
  [ "$(/usr/bin/stat -c %d:%i "$target")" = "$staged_identity" ] \
    || fail "private publication changed inode: $target"
  /usr/bin/rm -- "$staged" || fail "cannot retire private staging link"
  fsync_directory "$directory"
  validate_user_file "$target" 600
}

sha256_of() {
  digest=$(/usr/bin/sha256sum "$1") || fail "cannot hash $1"
  /usr/bin/printf '%s\n' "${digest%% *}"
}

validate_fixed_file() {
  validate_fixed_file_links "$1" "$2" "$3" 1 "$4"
}

validate_fixed_file_links() {
  file=$1
  expected_uid=$2
  expected_mode=$3
  expected_links=$4
  expected_sha=$5
  [ ! -L "$file" ] || fail "fixed file must not be a symlink: $file"
  [ -f "$file" ] || fail "fixed file is missing: $file"
  [ "$(/usr/bin/stat -c %u:%a:%h "$file")" \
    = "$expected_uid:$expected_mode:$expected_links" ] \
    || fail "fixed file owner/mode/link-count mismatch: $file"
  [ "$(sha256_of "$file")" = "$expected_sha" ] || fail "fixed file hash mismatch: $file"
}

validate_fixture_sources() {
  validate_fixed_file "$loader_source" "$uid" 755 "$LOADER_SHA256"
  validate_fixed_file "$high_dropin_source" "$uid" 644 "$HIGH_DROPIN_SHA256"
  validate_fixed_file "$max_dropin_source" "$uid" 644 "$MAX_DROPIN_SHA256"
  validate_fixed_file "$high_go_source" "$uid" 644 "$HIGH_GO_SHA256"
  validate_fixed_file "$max_go_source" "$uid" 644 "$MAX_GO_SHA256"
}

ensure_runtime_directory() {
  directory=$1
  expected_mode=$2
  if [ ! -e "$directory" ]; then
    /usr/bin/mkdir -m "0$expected_mode" -- "$directory" \
      || fail "cannot create runtime directory: $directory"
  fi
  validate_owned_directory "$directory"
  [ "$(/usr/bin/stat -c %a "$directory")" = "$expected_mode" ] \
    || fail "runtime directory mode mismatch: $directory"
}

stage_fixed_fixture() {
  source_file=$1
  target_file=$2
  target_directory=$3
  expected_mode=$4
  expected_sha=$5
  if [ -e "$target_file" ] || [ -L "$target_file" ]; then
    validate_fixed_file "$target_file" "$uid" "$expected_mode" "$expected_sha"
    return 0
  fi
  fixture_tmp=$(/usr/bin/mktemp "$target_directory/.pinvou-e2e.XXXXXX") \
    || fail "cannot stage fixed E2E fixture"
  /usr/bin/install -m "0$expected_mode" -- "$source_file" "$fixture_tmp" \
    || fail "cannot populate fixed E2E fixture"
  validate_fixed_file "$fixture_tmp" "$uid" "$expected_mode" "$expected_sha"
  fsync_file "$fixture_tmp"
  staged_identity=$(/usr/bin/stat -c %d:%i "$fixture_tmp") \
    || fail "cannot identify fixed E2E staging file"
  /usr/bin/ln -T -- "$fixture_tmp" "$target_file" \
    || fail "fixed E2E target appeared concurrently; refusing to overwrite it"
  fsync_directory "$target_directory"
  validate_fixed_file_links "$fixture_tmp" "$uid" "$expected_mode" 2 "$expected_sha"
  validate_fixed_file_links "$target_file" "$uid" "$expected_mode" 2 "$expected_sha"
  [ "$(/usr/bin/stat -c %d:%i "$target_file")" = "$staged_identity" ] \
    || fail "fixed E2E publication changed inode"
  /usr/bin/rm -- "$fixture_tmp" || fail "cannot retire fixed E2E staging link"
  fixture_tmp=
  fsync_directory "$target_directory"
  validate_fixed_file "$target_file" "$uid" "$expected_mode" "$expected_sha"
}

remove_fixed_file() {
  target_file=$1
  expected_mode=$2
  expected_sha=$3
  if [ ! -e "$target_file" ] && [ ! -L "$target_file" ]; then
    return 0
  fi
  validate_fixed_file "$target_file" "$uid" "$expected_mode" "$expected_sha"
  /usr/bin/rm -- "$target_file" || return 1
}

validate_evidence_file() {
  evidence_file=$1
  expected_mode=$2
  expected_cgroup=${3-}
  validate_user_file "$evidence_file" 600
  /usr/bin/python3 -I - "$evidence_file" "$expected_mode" "$expected_cgroup" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1])
expected_mode = sys.argv[2]
expected_cgroup = sys.argv[3]
value = json.loads(path.read_text(encoding="utf-8"))
if set(value) != {"schema", "mode", "pid", "cgroup"}:
    raise SystemExit("unexpected loader evidence fields")
if value["schema"] != 1 or value["mode"] != expected_mode:
    raise SystemExit("loader evidence identity mismatch")
if not isinstance(value["pid"], int) or value["pid"] <= 1:
    raise SystemExit("loader evidence PID is invalid")
if not isinstance(value["cgroup"], str) or not value["cgroup"].endswith("/pinvou3-app.service"):
    raise SystemExit("loader evidence cgroup is invalid")
if expected_cgroup and value["cgroup"] != expected_cgroup:
    raise SystemExit("loader evidence does not match the app ControlGroup")
PY
}

remove_e2e_assets() {
  changed=0
  if [ -e "$e2e_dropin_target" ] || [ -L "$e2e_dropin_target" ]; then
    observed_dropin_hash=$(sha256_of "$e2e_dropin_target" 2>/dev/null) || return 1
    case "$observed_dropin_hash" in
      "$HIGH_DROPIN_SHA256"|"$MAX_DROPIN_SHA256") ;;
      *) return 1 ;;
    esac
    remove_fixed_file "$e2e_dropin_target" 644 "$observed_dropin_hash" || return 1
    changed=1
  fi
  remove_fixed_file "$loader_target" 700 "$LOADER_SHA256" || return 1
  remove_fixed_file "$high_once_marker" 600 "$HIGH_ONCE_SHA256" || return 1
  remove_fixed_file "$max_once_marker" 600 "$MAX_ONCE_SHA256" || return 1
  remove_fixed_file "$high_go_marker" 600 "$HIGH_GO_SHA256" || return 1
  remove_fixed_file "$max_go_marker" 600 "$MAX_GO_SHA256" || return 1
  for evidence_spec in "$high_evidence:high" "$max_evidence:max"; do
    evidence_file=${evidence_spec%:*}
    evidence_mode=${evidence_spec##*:}
    if [ -e "$evidence_file" ] || [ -L "$evidence_file" ]; then
      validate_evidence_file "$evidence_file" "$evidence_mode" || return 1
      /usr/bin/rm -- "$evidence_file" || return 1
    fi
  done
  if [ "$changed" -eq 1 ]; then
    /usr/bin/systemctl --user daemon-reload >/dev/null 2>&1 || return 1
  fi
  /usr/bin/rmdir -- "$e2e_dropin_dir" >/dev/null 2>&1 || true
  if [ -d "$e2e_runtime_dir" ] && ! /usr/bin/rmdir -- "$e2e_runtime_dir" 2>/dev/null; then
    return 1
  fi
  return 0
}

assert_no_e2e_assets() {
  for target in \
    "$e2e_dropin_target" "$loader_target" \
    "$high_once_marker" "$max_once_marker" \
    "$high_go_marker" "$max_go_marker" \
    "$high_evidence" "$max_evidence"; do
    [ ! -e "$target" ] && [ ! -L "$target" ] \
      || fail "fixed E2E asset remains: $target"
  done
  [ ! -e "$e2e_runtime_dir" ] && [ ! -L "$e2e_runtime_dir" ] \
    || fail "dedicated E2E runtime directory remains"
}

fixed_prefix_absent() {
  directory=$1
  prefix=$2
  [ ! -L "$directory" ] || return 1
  [ -d "$directory" ] || return 0
  for candidate in "$directory"/"$prefix"*; do
    [ ! -e "$candidate" ] && [ ! -L "$candidate" ] || return 1
  done
  return 0
}

transaction_residue_absent() {
  for fixed_staging_dir in \
    "$profile_staging_dir" "$desktop_staging_dir" "$marker_staging_dir"; do
    [ ! -e "$fixed_staging_dir" ] && [ ! -L "$fixed_staging_dir" ] || return 1
  done
  fixed_prefix_absent "$e2e_state_dir" .baseline. || return 1
  fixed_prefix_absent "$e2e_state_dir" .purge. || return 1
  fixed_prefix_absent "${profile_target%/*}" .pinvou-profile. || return 1
  fixed_prefix_absent "${profile_target%/*}" .pinvou-quarantine- || return 1
  fixed_prefix_absent "${desktop_target%/*}" .pinvou-desktop. || return 1
  fixed_prefix_absent "${desktop_target%/*}" .pinvou-quarantine- || return 1
  fixed_prefix_absent "$profile_state_dir" .pinvou-marker. || return 1
  fixed_prefix_absent "$profile_state_dir" .pinvou-quarantine- || return 1
  fixed_prefix_absent "$e2e_runtime_dir" .pinvou-e2e. || return 1
  fixed_prefix_absent "$e2e_dropin_dir" .pinvou-e2e. || return 1
  return 0
}

assert_no_transaction_residue() {
  transaction_residue_absent \
    || fail "a fixed helper/E2E staging or quarantine residue remains"
}

unit_property() {
  /usr/bin/systemctl --user show "$1" --property="$2" --value
}

assert_captured_unit_state() {
  unit=$1
  expected_active=$2
  kind=$3
  observed_state=$(unit_property "$unit" ActiveState 2>/dev/null) || return 1
  case "$expected_active:$observed_state" in
    1:active|0:inactive) ;;
    *) return 1 ;;
  esac
  if [ "$kind" = service ]; then
    observed_pid=$(unit_property "$unit" MainPID 2>/dev/null) || return 1
    case "$expected_active:$observed_pid" in
      0:0) ;;
      1:0|1:''|1:*[!0-9]*) return 1 ;;
      1:*) ;;
      *) return 1 ;;
    esac
  elif [ "$kind" != socket ]; then
    return 1
  fi
  return 0
}

capture_initial_control_unit_states() {
  socket_state=$(unit_property "$SOCKET_UNIT" ActiveState) \
    || fail "cannot inspect the initial Supervisor socket state"
  supervisor_state=$(unit_property "$SUPERVISOR_UNIT" ActiveState) \
    || fail "cannot inspect the initial Supervisor service state"
  case "$socket_state" in
    active) socket_was_active=1 ;;
    inactive) socket_was_active=0 ;;
    *) fail "initial Supervisor socket must be exactly active or inactive: $socket_state" ;;
  esac
  case "$supervisor_state" in
    active) supervisor_was_active=1 ;;
    inactive) supervisor_was_active=0 ;;
    *) fail "initial Supervisor service must be exactly active or inactive: $supervisor_state" ;;
  esac
  assert_captured_unit_state "$SOCKET_UNIT" "$socket_was_active" socket \
    || fail "initial Supervisor socket state is internally inconsistent"
  assert_captured_unit_state "$SUPERVISOR_UNIT" "$supervisor_was_active" service \
    || fail "initial Supervisor service state is internally inconsistent"
  if [ "$supervisor_was_active" -eq 1 ] && [ "$socket_was_active" -eq 0 ]; then
    fail "initial Supervisor service is active without its required socket"
  fi
}

unit_duration_us() {
  unit=$1
  property=$2
  unit_property "$unit" "$property" | /usr/bin/python3 -I -c '
import re, sys
value = sys.stdin.read().strip()
match = re.fullmatch(r"([0-9]+(?:\.[0-9]+)?)(us|ms|s|min|h)", value)
if not match:
    raise SystemExit("systemd duration is not finite")
scale = {"us": 1, "ms": 1_000, "s": 1_000_000, "min": 60_000_000, "h": 3_600_000_000}
microseconds = float(match.group(1)) * scale[match.group(2)]
if not microseconds.is_integer():
    raise SystemExit("systemd duration is not an integral microsecond value")
print(int(microseconds))
'
}

wait_for_property() {
  unit=$1
  property=$2
  expected=$3
  attempts=$4
  index=0
  while [ "$index" -lt "$attempts" ]; do
    [ "$(unit_property "$unit" "$property" 2>/dev/null)" = "$expected" ] && return 0
    /usr/bin/sleep 1
    index=$((index + 1))
  done
  return 1
}

validate_deb() {
  deb_path=$1
  deb_directory=${deb_path%/*}
  deb_basename=${deb_path##*/}
  [ "$deb_directory" = "$home_dir" ] \
    || fail "deb must be directly under the login home"
  case "$deb_basename" in
    pinvou3_*_amd64.deb) ;;
    *) fail "deb basename is not a Pinvou amd64 artifact" ;;
  esac
  case "$deb_basename" in
    *[!A-Za-z0-9._-]*) fail "deb basename contains an unsafe character" ;;
  esac
  [ ! -L "$deb_path" ] || fail "deb must not be a symlink"
  [ -f "$deb_path" ] || fail "deb is missing"
  [ "$(/usr/bin/stat -c %u "$deb_path")" = "$uid" ] || fail "deb owner mismatch"
  permissions=$(/usr/bin/stat -c %A "$deb_path")
  case "$permissions" in
    ?????w*|????????w*) fail "deb is group/other writable" ;;
  esac
  [ "$(/usr/bin/dpkg-deb --field "$deb_path" Package)" = pinvou3 ] \
    || fail "deb package identity mismatch"
  [ "$(/usr/bin/dpkg-deb --field "$deb_path" Architecture)" = amd64 ] \
    || fail "deb architecture is not amd64"
}

deb_identity_snapshot() {
  snapshot_deb=$1
  /usr/bin/python3 -I - "$snapshot_deb" "$uid" <<'PY'
import hashlib
import os
import stat
import sys

path, expected_uid_text = sys.argv[1:]
try:
    expected_uid = int(expected_uid_text)
except ValueError:
    raise SystemExit("exact deb expected owner is invalid")

required_flags = ("O_CLOEXEC", "O_NOFOLLOW")
if any(not hasattr(os, name) for name in required_flags):
    raise SystemExit("exact deb no-follow snapshot flags are unavailable")
flags = os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW
try:
    fd = os.open(path, flags)
except OSError as error:
    raise SystemExit(f"cannot open exact deb without following links: {error}")

def identity(file_stat):
    return (
        file_stat.st_dev,
        file_stat.st_ino,
        file_stat.st_mode,
        file_stat.st_uid,
        file_stat.st_gid,
        file_stat.st_nlink,
        file_stat.st_size,
        file_stat.st_mtime_ns,
        file_stat.st_ctime_ns,
    )

try:
    before = os.fstat(fd)
    if not stat.S_ISREG(before.st_mode):
        raise SystemExit("exact deb is not a regular file")
    if before.st_uid != expected_uid:
        raise SystemExit("exact deb owner changed")
    if before.st_mode & 0o022:
        raise SystemExit("exact deb became group/other writable")
    digest = hashlib.sha256()
    while True:
        chunk = os.read(fd, 1024 * 1024)
        if not chunk:
            break
        digest.update(chunk)
    after = os.fstat(fd)
    if identity(before) != identity(after):
        raise SystemExit("exact deb changed while its snapshot was hashed")
    try:
        path_after = os.stat(path, follow_symlinks=False)
    except OSError as error:
        raise SystemExit(f"exact deb path disappeared during snapshot: {error}")
    if not stat.S_ISREG(path_after.st_mode) or identity(path_after) != identity(after):
        raise SystemExit("exact deb path no longer names the snapshotted regular inode")
    values = identity(after)
    print(" ".join(str(value) for value in values), digest.hexdigest())
finally:
    os.close(fd)
PY
}

payload_attestation() {
  attestation_mode=$1
  attestation_deb=$2
  expected_attestation=${3-}
  set -- \
    /usr/bin/pinvou3-tauri:755 \
    /usr/lib/pinvou3/supervisor/pinvou-supervisor:755 \
    /usr/lib/pinvou3/supervisor/pinvou-megabook-profile:755 \
    /usr/lib/systemd/user/pinvou3-supervisor.socket:644 \
    /usr/lib/systemd/user/pinvou3-supervisor.service:644 \
    /usr/lib/systemd/user/pinvou3-app.service:644 \
    /usr/lib/systemd/user/pinvou-qwen3-asr.service.d/50-pinvou-supervisor.conf:644 \
    /usr/share/pinvou3/supervisor/descriptors/pinvou-app-v1.json:644 \
    /usr/share/pinvou3/supervisor/descriptors/pinvou-asr-v1.json:644 \
    /usr/share/pinvou3/supervisor/profiles/megabook-canary.conf:644 \
    /usr/share/pinvou3/supervisor/profiles/pinvou3-megabook-canary.desktop:644 \
    /usr/share/applications/pinvou3.desktop:644
  /usr/bin/python3 -I - \
    "$attestation_mode" "$attestation_deb" "$expected_attestation" "$@" <<'PY'
import hashlib
import os
import posixpath
import re
import stat
import subprocess
import sys
import tarfile

mode, deb_path, expected_attestation, *spec_fields = sys.argv[1:]
if mode not in {"baseline", "installed"}:
    raise SystemExit("fixed payload attestation mode is invalid")
specs = {}
for field in spec_fields:
    path, mode_text = field.rsplit(":", 1)
    if not path.startswith("/") or path in specs or re.fullmatch(r"[0-7]{3}", mode_text) is None:
        raise SystemExit("fixed payload specification is invalid")
    specs[path] = int(mode_text, 8)
if len(specs) != 12:
    raise SystemExit("fixed payload specification count changed")

def canonical_manifest_digest(records):
    if set(records) != set(specs):
        missing = sorted(set(specs) - set(records))
        extra = sorted(set(records) - set(specs))
        raise SystemExit(f"fixed payload manifest mismatch: missing={missing}, extra={extra}")
    digest = hashlib.sha256(b"pinvou-install-attestation-v1\0")
    for path in sorted(records):
        file_mode, size, file_hash = records[path]
        digest.update(f"{path}\0{file_mode:04o}\0{size}\0{file_hash}\n".encode("ascii"))
    return digest.hexdigest()

def archive_records():
    records = {}
    process = subprocess.Popen(
        ["/usr/bin/dpkg-deb", "--fsys-tarfile", deb_path],
        stdout=subprocess.PIPE,
    )
    assert process.stdout is not None
    try:
        with tarfile.open(fileobj=process.stdout, mode="r|*") as archive:
            for member in archive:
                raw_name = member.name
                name = raw_name[2:] if raw_name.startswith("./") else raw_name
                normalized = posixpath.normpath(name)
                normalized_path = "/" + normalized.lstrip("/")
                if normalized_path in specs and (raw_name.startswith("/") or name != normalized):
                    raise SystemExit(
                        f"fixed payload has a non-canonical archive alias: {raw_name}"
                    )
                path = "/" + name if not name.startswith("/") else name
                if path not in specs:
                    continue
                if path in records:
                    raise SystemExit(f"fixed payload occurs more than once in deb: {path}")
                expected_mode = specs[path]
                if not member.isreg() or member.uid != 0 or member.gid != 0 \
                        or (member.mode & 0o7777) != expected_mode:
                    raise SystemExit(
                        f"fixed deb payload type/owner/mode mismatch: {path}"
                    )
                source = archive.extractfile(member)
                if source is None:
                    raise SystemExit(f"cannot read fixed deb payload: {path}")
                file_digest = hashlib.sha256()
                observed_size = 0
                while True:
                    block = source.read(1024 * 1024)
                    if not block:
                        break
                    observed_size += len(block)
                    file_digest.update(block)
                if observed_size != member.size:
                    raise SystemExit(f"fixed deb payload size changed while reading: {path}")
                records[path] = (expected_mode, observed_size, file_digest.hexdigest())
    except BaseException:
        process.stdout.close()
        process.wait()
        raise
    process.stdout.close()
    if process.wait() != 0:
        raise SystemExit("dpkg-deb could not stream the exact baseline payload")
    return records

expected_records = archive_records()
archive_attestation = canonical_manifest_digest(expected_records)
if mode == "baseline":
    print(archive_attestation)
    raise SystemExit(0)
if re.fullmatch(r"[0-9a-f]{64}", expected_attestation) is None:
    raise SystemExit("baseline payload attestation is malformed")
if archive_attestation != expected_attestation:
    raise SystemExit("exact deb payload no longer matches the baseline attestation")

actual_records = {}
open_flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
for path in sorted(specs):
    parent = "/"
    for component in path.strip("/").split("/")[:-1]:
        parent = os.path.join(parent, component)
        parent_stat = os.lstat(parent)
        if stat.S_ISLNK(parent_stat.st_mode) or not stat.S_ISDIR(parent_stat.st_mode) \
                or parent_stat.st_uid != 0 or parent_stat.st_gid != 0 \
                or stat.S_IMODE(parent_stat.st_mode) & 0o022:
            raise SystemExit(f"installed payload ancestor is unsafe: {parent}")
    before = os.lstat(path)
    expected_mode, expected_size, expected_hash = expected_records[path]
    if stat.S_ISLNK(before.st_mode) or not stat.S_ISREG(before.st_mode) \
            or before.st_uid != 0 or before.st_gid != 0 \
            or stat.S_IMODE(before.st_mode) != expected_mode or before.st_nlink != 1:
        raise SystemExit(f"installed payload type/owner/mode/link-count mismatch: {path}")
    if before.st_size != expected_size:
        raise SystemExit(f"installed payload size does not match the exact deb: {path}")
    descriptor = os.open(path, open_flags)
    try:
        opened = os.fstat(descriptor)
        if (opened.st_dev, opened.st_ino, opened.st_size) != (
            before.st_dev, before.st_ino, before.st_size
        ) or not stat.S_ISREG(opened.st_mode) or opened.st_uid != 0 or opened.st_gid != 0 \
                or stat.S_IMODE(opened.st_mode) != expected_mode or opened.st_nlink != 1:
            raise SystemExit(f"installed payload changed while opening it: {path}")
        file_digest = hashlib.sha256()
        while True:
            block = os.read(descriptor, 1024 * 1024)
            if not block:
                break
            file_digest.update(block)
    finally:
        os.close(descriptor)
    after = os.lstat(path)
    if (after.st_dev, after.st_ino, after.st_size, after.st_nlink) != (
        before.st_dev, before.st_ino, before.st_size, before.st_nlink
    ):
        raise SystemExit(f"installed payload changed while hashing it: {path}")
    observed_hash = file_digest.hexdigest()
    if observed_hash != expected_hash:
        raise SystemExit(f"installed payload hash does not match the exact deb: {path}")
    actual_records[path] = (expected_mode, expected_size, observed_hash)
if canonical_manifest_digest(actual_records) != archive_attestation:
    raise SystemExit("installed fixed payload attestation does not match the exact deb")
print(archive_attestation)
PY
}

control_md5sums_attestation() {
  control_mode=$1
  control_deb=$2
  expected_control_attestation=${3-}
  /usr/bin/python3 -I - \
    "$control_mode" "$control_deb" "$expected_control_attestation" \
    usr/bin/pinvou3-tauri \
    usr/lib/pinvou3/supervisor/pinvou-supervisor \
    usr/lib/pinvou3/supervisor/pinvou-megabook-profile \
    usr/lib/systemd/user/pinvou3-supervisor.socket \
    usr/lib/systemd/user/pinvou3-supervisor.service \
    usr/lib/systemd/user/pinvou3-app.service \
    usr/lib/systemd/user/pinvou-qwen3-asr.service.d/50-pinvou-supervisor.conf \
    usr/share/pinvou3/supervisor/descriptors/pinvou-app-v1.json \
    usr/share/pinvou3/supervisor/descriptors/pinvou-asr-v1.json \
    usr/share/pinvou3/supervisor/profiles/megabook-canary.conf \
    usr/share/pinvou3/supervisor/profiles/pinvou3-megabook-canary.desktop \
    usr/share/applications/pinvou3.desktop <<'PY'
import hashlib
import os
import posixpath
import re
import stat
import subprocess
import sys

mode, deb_path, expected_attestation, *required_paths = sys.argv[1:]
if mode not in {"baseline", "installed"} or len(required_paths) != 12:
    raise SystemExit("fixed control md5sums attestation specification is invalid")

result = subprocess.run(
    ["/usr/bin/dpkg-deb", "--info", deb_path, "md5sums"],
    stdout=subprocess.PIPE,
    check=False,
)
if result.returncode != 0:
    raise SystemExit("exact deb does not expose its control md5sums")
raw = result.stdout
if not raw or len(raw) > 16 * 1024 * 1024 or raw[-1:] != b"\n" or b"\0" in raw:
    raise SystemExit("exact deb control md5sums is empty, unbounded, or unterminated")

entries = {}
for index, line in enumerate(raw.splitlines(), 1):
    match = re.fullmatch(rb"([0-9a-f]{32})  ([^\r\n]+)", line)
    if match is None:
        raise SystemExit(f"exact deb control md5sums line {index} is malformed")
    path = match.group(2).decode("utf-8", "strict")
    if path.startswith("/") or posixpath.normpath(path) != path \
            or any(part in {"", ".", ".."} for part in path.split("/")):
        raise SystemExit(f"exact deb control md5sums path is unsafe: {path}")
    if path in entries:
        raise SystemExit(f"exact deb control md5sums path is duplicated: {path}")
    entries[path] = match.group(1).decode("ascii")
for path in required_paths:
    if path not in entries:
        raise SystemExit(f"fixed payload is absent from exact deb control md5sums: /{path}")

attestation = hashlib.sha256(raw).hexdigest()
if mode == "baseline":
    print(attestation)
    raise SystemExit(0)
if re.fullmatch(r"[0-9a-f]{64}", expected_attestation) is None \
        or attestation != expected_attestation:
    raise SystemExit("exact deb control md5sums no longer matches the baseline")

query = subprocess.run(
    ["/usr/bin/dpkg-query", "--control-path", "pinvou3", "md5sums"],
    stdout=subprocess.PIPE,
    check=False,
)
if query.returncode != 0:
    raise SystemExit("installed pinvou3 control md5sums path is unavailable")
try:
    control_path = query.stdout.decode("ascii", "strict").rstrip("\n")
except UnicodeDecodeError as error:
    raise SystemExit("installed pinvou3 control md5sums path is malformed") from error
if control_path != "/var/lib/dpkg/info/pinvou3.md5sums" \
        or query.stdout != (control_path + "\n").encode("ascii"):
    raise SystemExit("installed pinvou3 control md5sums path is not the fixed dpkg path")

parent = "/"
for component in control_path.strip("/").split("/")[:-1]:
    parent = os.path.join(parent, component)
    parent_stat = os.lstat(parent)
    if stat.S_ISLNK(parent_stat.st_mode) or not stat.S_ISDIR(parent_stat.st_mode) \
            or parent_stat.st_uid != 0 or parent_stat.st_gid != 0 \
            or stat.S_IMODE(parent_stat.st_mode) & 0o022:
        raise SystemExit(f"installed dpkg control ancestor is unsafe: {parent}")
before = os.lstat(control_path)
if stat.S_ISLNK(before.st_mode) or not stat.S_ISREG(before.st_mode) \
        or before.st_uid != 0 or before.st_gid != 0 \
        or stat.S_IMODE(before.st_mode) != 0o644 or before.st_nlink != 1:
    raise SystemExit("installed dpkg control md5sums type/owner/mode/link-count mismatch")
if before.st_size != len(raw):
    raise SystemExit("installed dpkg control md5sums size differs from the exact deb")
flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
descriptor = os.open(control_path, flags)
try:
    opened = os.fstat(descriptor)
    if (opened.st_dev, opened.st_ino, opened.st_size, opened.st_nlink) != (
        before.st_dev, before.st_ino, before.st_size, before.st_nlink
    ) or not stat.S_ISREG(opened.st_mode) or opened.st_uid != 0 or opened.st_gid != 0 \
            or stat.S_IMODE(opened.st_mode) != 0o644:
        raise SystemExit("installed dpkg control md5sums changed while opening it")
    blocks = []
    observed_size = 0
    while True:
        block = os.read(descriptor, 1024 * 1024)
        if not block:
            break
        observed_size += len(block)
        if observed_size > 16 * 1024 * 1024:
            raise SystemExit("installed dpkg control md5sums exceeded its bound")
        blocks.append(block)
finally:
    os.close(descriptor)
after = os.lstat(control_path)
if (after.st_dev, after.st_ino, after.st_size, after.st_nlink) != (
    before.st_dev, before.st_ino, before.st_size, before.st_nlink
):
    raise SystemExit("installed dpkg control md5sums changed while reading it")
if b"".join(blocks) != raw:
    raise SystemExit("installed dpkg control md5sums bytes differ from the exact deb")
print(attestation)
PY
}

control_archive_attestation() {
  archive_mode=$1
  archive_deb=$2
  expected_members=${3-}
  expected_fields=${4-}
  expected_generated_list=${5-}
  /usr/bin/python3 -I - \
    "$archive_mode" "$archive_deb" "$expected_members" "$expected_fields" \
    "$expected_generated_list" <<'PY'
import hashlib
import os
import posixpath
import re
import stat
import subprocess
import sys
import tarfile

mode, deb_path, expected_members, expected_fields, expected_generated_list = sys.argv[1:]
if mode not in {"baseline", "installed"}:
    raise SystemExit("fixed control archive attestation mode is invalid")

MAX_CONTROL_MEMBER_BYTES = 16 * 1024 * 1024
MAX_CONTROL_TOTAL_BYTES = 32 * 1024 * 1024
MAX_CONTROL_MEMBERS = 64
MAX_DATA_MEMBERS = 200_000
MAX_GENERATED_LIST_BYTES = 32 * 1024 * 1024
CONTROL_BASENAME = re.compile(r"[a-z0-9][a-z0-9.+-]{0,63}\Z")
TRACKED_FIELDS = (
    "Package",
    "Source",
    "Version",
    "Architecture",
    "Pre-Depends",
    "Depends",
    "Recommends",
    "Suggests",
    "Enhances",
    "Conflicts",
    "Breaks",
    "Replaces",
    "Provides",
    "Essential",
    "Protected",
    "Important",
    "Multi-Arch",
    "Built-Using",
    "Static-Built-Using",
    "Package-Type",
)


def stream_tar(command, consumer, label):
    process = subprocess.Popen(command, stdout=subprocess.PIPE)
    assert process.stdout is not None
    try:
        with tarfile.open(fileobj=process.stdout, mode="r|*") as archive:
            result = consumer(archive)
    except BaseException:
        process.stdout.close()
        process.wait()
        raise
    process.stdout.close()
    if process.wait() != 0:
        raise SystemExit(f"dpkg-deb could not stream the exact {label}")
    return result


def read_member(archive, member, bound, label):
    if member.size < 0 or member.size > bound:
        raise SystemExit(f"{label} size is outside its fixed bound: {member.name}")
    source = archive.extractfile(member)
    if source is None:
        raise SystemExit(f"cannot read {label}: {member.name}")
    digest = hashlib.sha256()
    blocks = []
    observed = 0
    while True:
        block = source.read(1024 * 1024)
        if not block:
            break
        observed += len(block)
        if observed > bound:
            raise SystemExit(f"{label} exceeded its fixed bound: {member.name}")
        digest.update(block)
        blocks.append(block)
    if observed != member.size:
        raise SystemExit(f"{label} changed while reading: {member.name}")
    return b"".join(blocks), digest.hexdigest()


def parse_control_fields(raw):
    try:
        text = raw.decode("utf-8", "strict")
    except UnicodeDecodeError as error:
        raise SystemExit("main control member is not valid UTF-8") from error
    if "\x00" in text or "\r" in text or not text.endswith("\n"):
        raise SystemExit("main control member has unsafe or unterminated bytes")
    fields = {}
    current = None
    trailing_blank = False
    for line_number, line in enumerate(text.splitlines(), 1):
        if not line:
            trailing_blank = True
            current = None
            continue
        if trailing_blank:
            raise SystemExit("main control member contains more than one stanza")
        if line[0] in " \t":
            if current is None:
                raise SystemExit(f"orphan main control continuation at line {line_number}")
            fields[current] += "\n" + line[1:]
            continue
        match = re.fullmatch(r"([A-Za-z0-9][A-Za-z0-9-]*):[ \t]*(.*)", line)
        if match is None:
            raise SystemExit(f"malformed main control field at line {line_number}")
        field_name = match.group(1).lower()
        if field_name in fields:
            raise SystemExit(f"duplicated main control field: {match.group(1)}")
        fields[field_name] = match.group(2)
        current = field_name
    for required in ("package", "version", "architecture"):
        if not fields.get(required):
            raise SystemExit(f"required main control field is absent: {required}")
    return fields


def normalized_field_value(value):
    return " ".join(value.split())


def tracked_field_records(fields):
    records = {}
    for field in TRACKED_FIELDS:
        value = fields.get(field.lower())
        if value is None:
            continue
        normalized = normalized_field_value(value)
        if not normalized:
            raise SystemExit(f"tracked main control field is empty: {field}")
        records[field] = normalized
    return records


def digest_control_members(records):
    digest = hashlib.sha256(b"pinvou-control-members-v1\0")
    for name in sorted(records):
        member_mode, size, member_hash, _ = records[name]
        digest.update(f"{name}\0{member_mode:04o}\0{size}\0{member_hash}\n".encode("ascii"))
    return digest.hexdigest()


def digest_control_fields(records):
    digest = hashlib.sha256(b"pinvou-control-fields-v1\0")
    for field in TRACKED_FIELDS:
        if field in records:
            digest.update(field.encode("ascii") + b"\0")
            digest.update(records[field].encode("utf-8") + b"\n")
    return digest.hexdigest()


def consume_control_archive(archive):
    root_seen = False
    control_raw = None
    records = {}
    total_size = 0
    count = 0
    for member in archive:
        count += 1
        if count > MAX_CONTROL_MEMBERS:
            raise SystemExit("control archive member count exceeded its fixed bound")
        if member.name == ".":
            if root_seen or not member.isdir() or member.uid != 0 or member.gid != 0 \
                    or (member.mode & 0o7777) != 0o755:
                raise SystemExit("control archive root directory metadata is invalid")
            root_seen = True
            continue
        if not member.name.startswith("./"):
            raise SystemExit(f"control archive member is not canonical: {member.name}")
        name = member.name[2:]
        if CONTROL_BASENAME.fullmatch(name) is None or "/" in name:
            raise SystemExit(f"control archive member is not a fixed basename: {member.name}")
        if name == "control" and control_raw is not None or name in records:
            raise SystemExit(f"control archive member is duplicated: {name}")
        if not member.isreg() or member.uid != 0 or member.gid != 0:
            raise SystemExit(f"control archive member type/owner is invalid: {name}")
        member_mode = member.mode & 0o7777
        if member_mode not in {0o644, 0o755}:
            raise SystemExit(f"control archive member mode is not fixed: {name}")
        raw, member_hash = read_member(
            archive, member, MAX_CONTROL_MEMBER_BYTES, "control archive member"
        )
        total_size += len(raw)
        if total_size > MAX_CONTROL_TOTAL_BYTES:
            raise SystemExit("control archive content exceeded its fixed bound")
        if name == "control":
            if member_mode != 0o644:
                raise SystemExit("main control member mode is not 0644")
            control_raw = raw
        else:
            records[name] = (member_mode, len(raw), member_hash, raw)
    if not root_seen or control_raw is None:
        raise SystemExit("control archive root or main control member is absent")
    if "md5sums" not in records:
        raise SystemExit("control archive does not contain md5sums")
    return records, tracked_field_records(parse_control_fields(control_raw))


def consume_data_archive(archive):
    lines = []
    seen = set()
    count = 0
    total = 0
    for member in archive:
        count += 1
        if count > MAX_DATA_MEMBERS:
            raise SystemExit("data archive member count exceeded its fixed bound")
        if member.uid != 0 or member.gid != 0:
            raise SystemExit(f"data archive member is not root-owned: {member.name}")
        if not (member.isdir() or member.isreg() or member.issym() or member.islnk()):
            raise SystemExit(f"data archive member type is unsupported: {member.name}")
        if member.name == ".":
            if not member.isdir():
                raise SystemExit("data archive root member is not a directory")
            canonical = "."
        else:
            if not member.name.startswith("./"):
                raise SystemExit(f"data archive member is not canonical: {member.name}")
            canonical = member.name[2:]
            if not canonical or canonical.startswith("/") or "\n" in canonical \
                    or posixpath.normpath(canonical) != canonical \
                    or any(part in {"", ".", ".."} for part in canonical.split("/")):
                raise SystemExit(f"data archive member path is unsafe: {member.name}")
        if canonical in seen:
            raise SystemExit(f"data archive member is duplicated: {member.name}")
        seen.add(canonical)
        line = ("/." if canonical == "." else "/" + canonical).encode("utf-8") + b"\n"
        total += len(line)
        if total > MAX_GENERATED_LIST_BYTES:
            raise SystemExit("generated dpkg list bytes exceeded their fixed bound")
        lines.append(line)
    if "." not in seen:
        raise SystemExit("data archive root member is absent")
    return b"".join(lines)


control_records, field_records = stream_tar(
    ["/usr/bin/dpkg-deb", "--ctrl-tarfile", deb_path],
    consume_control_archive,
    "control archive",
)
generated_list = stream_tar(
    ["/usr/bin/dpkg-deb", "--fsys-tarfile", deb_path],
    consume_data_archive,
    "data archive",
)
members_attestation = digest_control_members(control_records)
fields_attestation = digest_control_fields(field_records)
generated_list_attestation = hashlib.sha256(generated_list).hexdigest()

if mode == "baseline":
    print(f"{members_attestation}:{fields_attestation}:{generated_list_attestation}")
    raise SystemExit(0)
for label, expected, observed in (
    ("control members", expected_members, members_attestation),
    ("control fields", expected_fields, fields_attestation),
    ("generated dpkg list", expected_generated_list, generated_list_attestation),
):
    if re.fullmatch(r"[0-9a-f]{64}", expected) is None or expected != observed:
        raise SystemExit(f"exact deb {label} no longer match the baseline")


def verify_installed_file(info_fd, basename, expected_mode, expected_size, expected_hash, expected_raw):
    display_path = "/var/lib/dpkg/info/" + basename
    before = os.stat(basename, dir_fd=info_fd, follow_symlinks=False)
    if not stat.S_ISREG(before.st_mode) or before.st_uid != 0 or before.st_gid != 0 \
            or stat.S_IMODE(before.st_mode) != expected_mode or before.st_nlink != 1:
        raise SystemExit(f"installed dpkg control member metadata mismatch: {display_path}")
    if before.st_size != expected_size:
        raise SystemExit(f"installed dpkg control member size mismatch: {display_path}")
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(basename, flags, dir_fd=info_fd)
    try:
        opened = os.fstat(descriptor)
        if (opened.st_dev, opened.st_ino, opened.st_size, opened.st_nlink) != (
            before.st_dev, before.st_ino, before.st_size, before.st_nlink
        ) or not stat.S_ISREG(opened.st_mode) or opened.st_uid != 0 or opened.st_gid != 0 \
                or stat.S_IMODE(opened.st_mode) != expected_mode:
            raise SystemExit(f"installed dpkg control member changed while opening: {display_path}")
        digest = hashlib.sha256()
        blocks = []
        observed_size = 0
        while True:
            block = os.read(descriptor, 1024 * 1024)
            if not block:
                break
            observed_size += len(block)
            if observed_size > expected_size:
                raise SystemExit(f"installed dpkg control member exceeded its bound: {display_path}")
            digest.update(block)
            blocks.append(block)
    finally:
        os.close(descriptor)
    after = os.stat(basename, dir_fd=info_fd, follow_symlinks=False)
    if (after.st_dev, after.st_ino, after.st_size, after.st_nlink) != (
        before.st_dev, before.st_ino, before.st_size, before.st_nlink
    ):
        raise SystemExit(f"installed dpkg control member changed while reading: {display_path}")
    if observed_size != expected_size or digest.hexdigest() != expected_hash \
            or b"".join(blocks) != expected_raw:
        raise SystemExit(f"installed dpkg control member bytes mismatch: {display_path}")


control_list_result = subprocess.run(
    ["/usr/bin/dpkg-query", "--control-list", "pinvou3"],
    stdout=subprocess.PIPE,
    check=False,
)
if control_list_result.returncode != 0 or len(control_list_result.stdout) > 65536:
    raise SystemExit("installed pinvou3 control member list is unavailable or unbounded")
if control_list_result.stdout and not control_list_result.stdout.endswith(b"\n"):
    raise SystemExit("installed pinvou3 control member list is unterminated")
try:
    control_list_names = [line.decode("ascii", "strict")
                          for line in control_list_result.stdout.splitlines()]
except UnicodeDecodeError as error:
    raise SystemExit("installed pinvou3 control member list is not ASCII") from error
if len(control_list_names) != len(set(control_list_names)):
    raise SystemExit("installed pinvou3 control member list contains duplicates")
for name in control_list_names:
    if CONTROL_BASENAME.fullmatch(name) is None:
        raise SystemExit(f"installed pinvou3 control member name is unsafe: {name}")

# dpkg consumes the main control stanza into status and conffiles into its generated database
# file. Every other exact control-tar member must be returned by --control-list, with no extras.
listed_archive_names = set(control_records) - {"conffiles"}
if set(control_list_names) != listed_archive_names:
    raise SystemExit(
        "installed pinvou3 control member set differs from the exact control archive: "
        f"expected={sorted(listed_archive_names)}, observed={sorted(control_list_names)}"
    )

info_dir = "/var/lib/dpkg/info"
parent = "/"
for component in info_dir.strip("/").split("/"):
    parent = os.path.join(parent, component)
    parent_stat = os.lstat(parent)
    if stat.S_ISLNK(parent_stat.st_mode) or not stat.S_ISDIR(parent_stat.st_mode) \
            or parent_stat.st_uid != 0 or parent_stat.st_gid != 0 \
            or stat.S_IMODE(parent_stat.st_mode) & 0o022:
        raise SystemExit(f"installed dpkg control ancestor is unsafe: {parent}")
info_lstat = os.lstat(info_dir)
if stat.S_ISLNK(info_lstat.st_mode) or not stat.S_ISDIR(info_lstat.st_mode) \
        or info_lstat.st_uid != 0 or info_lstat.st_gid != 0 \
        or stat.S_IMODE(info_lstat.st_mode) & 0o022:
    raise SystemExit("installed dpkg info directory metadata is unsafe")
directory_flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) \
    | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_DIRECTORY", 0)
info_fd = os.open(info_dir, directory_flags)
try:
    opened_info = os.fstat(info_fd)
    if (opened_info.st_dev, opened_info.st_ino) != (info_lstat.st_dev, info_lstat.st_ino):
        raise SystemExit("installed dpkg info directory changed while opening")
    expected_basenames = {"pinvou3." + name for name in listed_archive_names}
    expected_basenames.add("pinvou3.list")
    if "conffiles" in control_records:
        expected_basenames.add("pinvou3.conffiles")
    observed_basenames = {
        name
        for name in os.listdir(info_fd)
        if name.startswith("pinvou3.") or name.startswith("pinvou3:")
    }
    if observed_basenames != expected_basenames:
        raise SystemExit(
            "installed dpkg control database has an unexpected complete member set: "
            f"expected={sorted(expected_basenames)}, observed={sorted(observed_basenames)}"
        )
    for name in sorted(listed_archive_names):
        member_mode, size, member_hash, raw = control_records[name]
        verify_installed_file(
            info_fd, "pinvou3." + name, member_mode, size, member_hash, raw
        )
    if "conffiles" in control_records:
        member_mode, size, member_hash, raw = control_records["conffiles"]
        verify_installed_file(
            info_fd, "pinvou3.conffiles", member_mode, size, member_hash, raw
        )
    verify_installed_file(
        info_fd,
        "pinvou3.list",
        0o644,
        len(generated_list),
        generated_list_attestation,
        generated_list,
    )
finally:
    os.close(info_fd)

status_result = subprocess.run(
    ["/usr/bin/dpkg-query", "--status", "pinvou3"],
    stdout=subprocess.PIPE,
    check=False,
)
if status_result.returncode != 0 or len(status_result.stdout) > 4 * 1024 * 1024:
    raise SystemExit("installed main control status stanza is unavailable or unbounded")
installed_status_fields = parse_control_fields(status_result.stdout)
if normalized_field_value(installed_status_fields.get("status", "")) != "install ok installed":
    raise SystemExit("installed main control status is not install ok installed")
installed_field_records = tracked_field_records(installed_status_fields)
if installed_field_records != field_records \
        or digest_control_fields(installed_field_records) != fields_attestation:
    raise SystemExit("installed main control key fields differ from the exact deb")
PY
}

baseline() {
  [ "$#" -eq 1 ] || fail "baseline requires one exact deb path"
  deb_path=$1
  validate_deb "$deb_path"
  if [ "$(/usr/bin/dpkg-query -W -f='${Status}' pinvou3 2>/dev/null || true)" \
    = 'install ok installed' ]; then
    fail "pinvou3 is already installed; baseline must precede sudo install"
  fi
  [ "$(unit_property "$ASR_UNIT" ActiveState)" = active ] \
    || fail "ASR must be active when the pre-install baseline is captured"
  asr_invocation=$(unit_property "$ASR_UNIT" InvocationID)
  case "$asr_invocation" in
    *[!0-9a-f]*|'') fail "ASR baseline InvocationID is invalid" ;;
  esac
  [ "${#asr_invocation}" -eq 32 ] || fail "ASR baseline InvocationID is invalid"

  ensure_state_directory
  recover_all_staging_orphans || fail "cannot safely recover a prior E2E staging orphan"
  assert_no_transaction_residue
  assert_no_e2e_assets
  deb_snapshot_before=$(deb_identity_snapshot "$deb_path") \
    || fail "cannot capture the exact deb pre-attestation snapshot"
  deb_snapshot_sha=${deb_snapshot_before##* }
  case "$deb_snapshot_sha" in
    *[!0-9a-f]*|'') fail "exact deb pre-attestation snapshot is malformed" ;;
  esac
  [ "${#deb_snapshot_sha}" -eq 64 ] \
    || fail "exact deb pre-attestation snapshot is malformed"
  deb_version=$(/usr/bin/dpkg-deb --field "$deb_path" Version) \
    || fail "cannot read the exact deb version"
  [ -n "$deb_version" ] || fail "exact deb version is empty"
  deb_attestation=$(payload_attestation baseline "$deb_path") \
    || fail "cannot attest the exact deb install payload"
  control_attestation=$(control_md5sums_attestation baseline "$deb_path") \
    || fail "cannot attest the exact deb control md5sums"
  control_archive_line=$(control_archive_attestation baseline "$deb_path") \
    || fail "cannot attest the exact deb control archive"
  previous_ifs=$IFS
  IFS=:
  set -- $control_archive_line
  IFS=$previous_ifs
  [ "$#" -eq 3 ] || fail "exact deb control archive attestation is malformed"
  control_members_attestation=$1
  control_fields_attestation=$2
  generated_list_attestation=$3
  for control_digest in \
    "$control_members_attestation" "$control_fields_attestation" \
    "$generated_list_attestation"; do
    case "$control_digest" in
      *[!0-9a-f]*|'') fail "exact deb control archive attestation is malformed" ;;
    esac
    [ "${#control_digest}" -eq 64 ] \
      || fail "exact deb control archive attestation is malformed"
  done
  deb_snapshot_after=$(deb_identity_snapshot "$deb_path") \
    || fail "cannot capture the exact deb post-attestation snapshot"
  [ "$deb_snapshot_after" = "$deb_snapshot_before" ] \
    || fail "exact deb changed during the baseline attestation sequence"
  baseline_tmp=$(/usr/bin/mktemp "$e2e_state_dir/.baseline.XXXXXX") \
    || fail "cannot stage baseline"
  /usr/bin/printf '%s\n' \
    'schema=pinvou-megabook-e2e-v3' \
    "deb_path=$deb_path" \
    "deb_sha256=$deb_snapshot_sha" \
    "deb_version=$deb_version" \
    'deb_architecture=amd64' \
    "deb_install_attestation_sha256=$deb_attestation" \
    "deb_control_md5sums_sha256=$control_attestation" \
    "deb_control_members_sha256=$control_members_attestation" \
    "deb_control_fields_sha256=$control_fields_attestation" \
    "deb_generated_list_sha256=$generated_list_attestation" \
    "asr_invocation_id=$asr_invocation" >"$baseline_tmp"
  validate_user_file "$baseline_tmp" 600
  if [ ! -e "$baseline_file" ] && [ ! -L "$baseline_file" ]; then
    publish_private_staged_file "$baseline_tmp" "$baseline_file" "$e2e_state_dir"
    baseline_tmp=
  elif [ ! -L "$baseline_file" ] && [ -f "$baseline_file" ] \
      && validate_user_file "$baseline_file" 600 \
      && /usr/bin/cmp -s -- "$baseline_tmp" "$baseline_file"; then
    fsync_file "$baseline_tmp"
    /usr/bin/rm -- "$baseline_tmp" || fail "cannot retire duplicate baseline staging file"
    baseline_tmp=
    fsync_directory "$e2e_state_dir"
  else
    /usr/bin/rm -f -- "$baseline_tmp" || true
    baseline_tmp=
    fsync_directory "$e2e_state_dir"
    fail "a different baseline already exists"
  fi
  /usr/bin/printf '%s\n' \
    "baseline-ready sha256=$deb_snapshot_sha asrInvocationID=$asr_invocation"
}

baseline_value() {
  key=$1
  value=$(/usr/bin/awk -F= -v key="$key" '$1 == key { sub(/^[^=]*=/, ""); print; count += 1 } END { if (count != 1) exit 1 }' "$baseline_file") \
    || fail "baseline key is missing or duplicated: $key"
  [ -n "$value" ] || fail "baseline key is empty: $key"
  /usr/bin/printf '%s\n' "$value"
}

load_and_verify_installed_package() {
  validate_user_file "$baseline_file" 600
  [ "$(baseline_value schema)" = pinvou-megabook-e2e-v3 ] || fail "baseline schema mismatch"
  deb_path=$(baseline_value deb_path)
  validate_deb "$deb_path"
  deb_snapshot_before=$(deb_identity_snapshot "$deb_path") \
    || fail "cannot capture the exact deb pre-verification snapshot"
  deb_snapshot_sha=${deb_snapshot_before##* }
  case "$deb_snapshot_sha" in
    *[!0-9a-f]*|'') fail "exact deb pre-verification snapshot is malformed" ;;
  esac
  [ "${#deb_snapshot_sha}" -eq 64 ] \
    || fail "exact deb pre-verification snapshot is malformed"
  [ "$deb_snapshot_sha" = "$(baseline_value deb_sha256)" ] \
    || fail "deb changed after baseline"
  deb_version=$(/usr/bin/dpkg-deb --field "$deb_path" Version) \
    || fail "cannot read the exact deb version"
  [ "$deb_version" = "$(baseline_value deb_version)" ] \
    || fail "deb version changed after baseline"
  [ "$(/usr/bin/dpkg-query -W -f='${Status}' pinvou3 2>/dev/null || true)" \
    = 'install ok installed' ] || fail "pinvou3 is not installed"
  [ "$(/usr/bin/dpkg-query -W -f='${Architecture}' pinvou3)" = amd64 ] \
    || fail "installed pinvou3 architecture mismatch"
  [ "$(/usr/bin/dpkg-query -W -f='${Version}' pinvou3)" = "$(baseline_value deb_version)" ] \
    || fail "installed pinvou3 version mismatch"
  [ "$(baseline_value deb_architecture)" = amd64 ] \
    || fail "baseline deb architecture mismatch"
  expected_attestation=$(baseline_value deb_install_attestation_sha256)
  case "$expected_attestation" in
    *[!0-9a-f]*|'') fail "baseline payload attestation is malformed" ;;
  esac
  [ "${#expected_attestation}" -eq 64 ] \
    || fail "baseline payload attestation is malformed"
  expected_control_attestation=$(baseline_value deb_control_md5sums_sha256)
  case "$expected_control_attestation" in
    *[!0-9a-f]*|'') fail "baseline control md5sums attestation is malformed" ;;
  esac
  [ "${#expected_control_attestation}" -eq 64 ] \
    || fail "baseline control md5sums attestation is malformed"
  expected_control_members=$(baseline_value deb_control_members_sha256)
  expected_control_fields=$(baseline_value deb_control_fields_sha256)
  expected_generated_list=$(baseline_value deb_generated_list_sha256)
  for expected_control_digest in \
    "$expected_control_members" "$expected_control_fields" "$expected_generated_list"; do
    case "$expected_control_digest" in
      *[!0-9a-f]*|'') fail "baseline control archive attestation is malformed" ;;
    esac
    [ "${#expected_control_digest}" -eq 64 ] \
      || fail "baseline control archive attestation is malformed"
  done
  control_archive_attestation \
    installed "$deb_path" "$expected_control_members" "$expected_control_fields" \
    "$expected_generated_list" >/dev/null \
    || fail "installed dpkg control behavior does not match the exact baseline deb"
  control_md5sums_attestation \
    installed "$deb_path" "$expected_control_attestation" >/dev/null \
    || fail "installed dpkg control md5sums does not match the exact baseline deb"
  if dpkg_verify_output=$(/usr/bin/dpkg --verify pinvou3); then
    [ -z "$dpkg_verify_output" ] \
      || fail "dpkg verification reported an installed pinvou3 payload difference"
  else
    fail "dpkg could not verify the installed pinvou3 payload"
  fi
  payload_attestation installed "$deb_path" "$expected_attestation" >/dev/null \
    || fail "installed payload does not match the exact baseline deb"
  deb_snapshot_after=$(deb_identity_snapshot "$deb_path") \
    || fail "cannot capture the exact deb post-verification snapshot"
  [ "$deb_snapshot_after" = "$deb_snapshot_before" ] \
    || fail "exact deb changed during installed-package verification"
}

load_and_verify_baseline() {
  load_and_verify_installed_package
  baseline_asr_invocation=$(baseline_value asr_invocation_id)
  [ "$(unit_property "$ASR_UNIT" InvocationID)" = "$baseline_asr_invocation" ] \
    || fail "postinst changed the running ASR InvocationID"
}

capture_append_offset() {
  file=$1
  maximum_bytes=$2
  if [ ! -e "$file" ] && [ ! -L "$file" ]; then
    /usr/bin/printf '%s\n' \
      0:0:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
    return 0
  fi
  validate_user_file "$file" 600
  size=$(/usr/bin/stat -c %s "$file") || fail "cannot size append-only evidence: $file"
  case "$size" in ''|*[!0-9]*) fail "append-only evidence size is invalid: $file" ;; esac
  [ "$size" -le "$maximum_bytes" ] \
    || fail "append-only evidence is too large for a bounded E2E window: $file"
  /usr/bin/python3 -I - "$file" "$size" <<'PY'
import hashlib, pathlib, sys
path = pathlib.Path(sys.argv[1])
size = int(sys.argv[2])
raw = path.read_bytes()
if len(raw) != size:
    raise SystemExit("append-only evidence changed during baseline capture")
if size and raw[-1:] != b"\n":
    raise SystemExit("append-only evidence has an unterminated committed tail")
print(f"{size}:{path.stat().st_ino}:{hashlib.sha256(raw).hexdigest()}")
PY
}

control_launch_record() {
  field=$1
  offset=$2
  /usr/bin/python3 -I - "$control_ledger" "$offset" "$field" <<'PY'
import hashlib, json, pathlib, sys
path = pathlib.Path(sys.argv[1])
token = sys.argv[2]
field = sys.argv[3]
raw = path.read_bytes()
parts = token.split(":")
if len(parts) != 3:
    raise SystemExit("control ledger append token is invalid")
offset, inode, prefix_hash = int(parts[0]), int(parts[1]), parts[2]
if inode and path.stat().st_ino != inode:
    raise SystemExit("control ledger inode changed")
if hashlib.sha256(raw[:offset]).hexdigest() != prefix_hash:
    raise SystemExit("control ledger prefix changed")
if offset < 0 or offset > len(raw) or (offset and raw[offset - 1:offset] != b"\n"):
    raise SystemExit("control ledger append boundary changed")
events = []
for index, frame in enumerate(raw[offset:].splitlines(), 1):
    try:
        events.append(json.loads(frame))
    except Exception as error:
        raise SystemExit(f"control ledger tail frame {index} is invalid: {error}")
candidates = []
for event in events:
    if event.get("event") != "control_completed_tombstone":
        continue
    fingerprint = event.get("fingerprint") or {}
    receipt = event.get("receipt") or {}
    if fingerprint.get("target") != "pinvou_app" or fingerprint.get("action") != "launch":
        continue
    request_id = fingerprint.get("request_id")
    expected = {
        "request_id": request_id,
        "target": "pinvou_app",
        "descriptor_revision": "pinvou-app-descriptor-v1",
        "expected_instance_generation": None,
        "action": "launch",
    }
    if fingerprint != expected:
        raise SystemExit("hardened launch fingerprint is not the fixed descriptor")
    if any(receipt.get(key) != value for key, value in expected.items()):
        raise SystemExit("hardened launch receipt does not match its fingerprint")
    if receipt.get("outcome") != "applied":
        raise SystemExit("hardened client did not perform the attributable first launch")
    candidates.append((request_id, receipt))
if len(candidates) != 1:
    raise SystemExit("expected exactly one hardened launch completion in this E2E window")
request_id, receipt = candidates[0]
if field == "request_id":
    print(request_id)
elif field == "receipt":
    print(json.dumps(receipt, sort_keys=True, separators=(",", ":")))
else:
    raise SystemExit("fixed control record field is invalid")
PY
}

assert_receipt() {
  expected_target=$1
  expected_action=$2
  shift 2
  "$@" | /usr/bin/python3 -I -c '
import json, sys
target, action = sys.argv[1:3]
receipt = json.load(sys.stdin)
if receipt.get("target") != target or receipt.get("action") != action:
    raise SystemExit("receipt target/action mismatch")
if receipt.get("outcome") != "reconciled":
    raise SystemExit("receipt was not reconciled: " + str(receipt.get("outcome")))
' "$expected_target" "$expected_action"
}

send_fixed_launch() {
  request_id=$1
  /usr/bin/python3 -I - "$request_id" <<'PY'
import json, os, socket, sys
request = {
    "protocol_version": 2,
    "request_id": sys.argv[1],
    "target": "pinvou_app",
    "descriptor_revision": "pinvou-app-descriptor-v1",
    "expected_instance_generation": None,
    "action": "launch",
}
path = f"/run/user/{os.geteuid()}/pinvou-supervisor/control.sock"
client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
client.settimeout(8)
client.connect(path)
client.sendall(json.dumps(request, separators=(",", ":")).encode() + b"\n")
response = bytearray()
while b"\n" not in response and len(response) <= 32768:
    block = client.recv(8192)
    if not block:
        break
    response.extend(block)
if not response.endswith(b"\n") or len(response) > 32768:
    raise SystemExit("bounded Supervisor response was incomplete")
receipt = json.loads(response[:-1])
if receipt.get("request_id") != request["request_id"]:
    raise SystemExit("Supervisor response request id mismatch")
if receipt.get("target") != "pinvou_app" or receipt.get("action") != "launch":
    raise SystemExit("Supervisor response target/action mismatch")
print(json.dumps(receipt, sort_keys=True, separators=(",", ":")))
PY
}

send_same_uid_asr_stop_negative() {
  request_id=$1
  expected_generation=$2
  /usr/bin/python3 -I - "$request_id" "$expected_generation" <<'PY'
import json, os, socket, sys
request = {
    "protocol_version": 2,
    "request_id": sys.argv[1],
    "target": "pinvou_asr",
    "descriptor_revision": "pinvou-asr-descriptor-v1",
    "expected_instance_generation": sys.argv[2],
    "action": "stop",
}
path = f"/run/user/{os.geteuid()}/pinvou-supervisor/control.sock"
client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
client.settimeout(8)
client.connect(path)
client.sendall(json.dumps(request, separators=(",", ":")).encode() + b"\n")
response = bytearray()
while b"\n" not in response and len(response) <= 32768:
    block = client.recv(8192)
    if not block:
        break
    response.extend(block)
if not response.endswith(b"\n") or len(response) > 32768:
    raise SystemExit("bounded Supervisor negative response was incomplete")
receipt = json.loads(response[:-1])
for key, value in request.items():
    if key == "protocol_version":
        continue
    if receipt.get(key) != value:
        raise SystemExit("negative response does not match the fixed request")
if receipt.get("protocol_version") != 2 or receipt.get("outcome") != "rejected":
    raise SystemExit("same-UID non-App client was not rejected")
if receipt.get("observation") is not None:
    raise SystemExit("pre-authorization rejection unexpectedly carried an observation")
if receipt.get("detail") != "control caller PID is not pinvou3-app.service MainPID":
    raise SystemExit("same-UID rejection did not prove the App MainPID boundary")
PY
}

validate_effective_app_profile() {
  [ "$(unit_property "$APP_UNIT" FragmentPath)" = /usr/lib/systemd/user/pinvou3-app.service ] \
    || fail "app FragmentPath mismatch"
  dropins=$(unit_property "$APP_UNIT" DropInPaths) || fail "cannot inspect app DropInPaths"
  if [ -e "$e2e_dropin_target" ] || [ -L "$e2e_dropin_target" ]; then
    /usr/bin/printf '%s' "$dropins" | /usr/bin/python3 -I -c '
import sys
paths = sys.stdin.read().split()
expected = sys.argv[1:]
if len(paths) != len(expected) or set(paths) != set(expected):
    raise SystemExit("effective app DropInPaths is not the fixed profile plus E2E loader")
' "$profile_target" "$e2e_dropin_target" \
      || fail "effective app DropInPaths is not the fixed profile plus E2E loader"
  else
    /usr/bin/printf '%s' "$dropins" | /usr/bin/python3 -I -c '
import sys
if sys.stdin.read().split() != [sys.argv[1]]:
    raise SystemExit("effective app DropInPaths is not the fixed profile")
' "$profile_target" || fail "effective app DropInPaths is not the fixed profile"
  fi
  [ "$(unit_property "$APP_UNIT" MemoryAccounting)" = yes ] \
    || fail "app MemoryAccounting mismatch"
  [ "$(unit_property "$APP_UNIT" MemoryHigh)" = 4294967296 ] || fail "app MemoryHigh mismatch"
  [ "$(unit_property "$APP_UNIT" MemoryMax)" = 8589934592 ] || fail "app MemoryMax mismatch"
  [ "$(unit_property "$APP_UNIT" MemorySwapMax)" = 2147483648 ] \
    || fail "app MemorySwapMax mismatch"
  [ "$(unit_property "$APP_UNIT" OOMPolicy)" = kill ] || fail "app OOMPolicy mismatch"
  [ "$(unit_property "$APP_UNIT" KillMode)" = control-group ] || fail "app KillMode mismatch"
  [ "$(unit_property "$APP_UNIT" TasksMax)" = 512 ] || fail "app TasksMax mismatch"
  [ "$(unit_property "$APP_UNIT" Restart)" = on-failure ] || fail "app Restart mismatch"
  [ "$(unit_duration_us "$APP_UNIT" RestartUSec)" = 15000000 ] \
    || fail "app RestartUSec mismatch"
  [ "$(unit_duration_us "$APP_UNIT" StartLimitIntervalUSec)" = 300000000 ] \
    || fail "app StartLimitIntervalUSec mismatch"
  [ "$(unit_property "$APP_UNIT" StartLimitBurst)" = 3 ] \
    || fail "app StartLimitBurst mismatch"
  unit_property "$APP_UNIT" Environment | /usr/bin/grep -Fq PINVOU_RESOURCE_PROFILE=megabook-canary-v1 \
    || fail "app profile identity environment is missing"
  unit_property "$APP_UNIT" ExecStopPost | /usr/bin/python3 -I -c '
import re, sys
value = sys.stdin.read().strip()
commands = re.findall(r"\{([^{}]*)\}", value)
if len(commands) != 1:
    raise SystemExit("app must have exactly one ExecStopPost command")
fields = {}
for segment in commands[0].split(";"):
    if "=" not in segment:
        continue
    key, field = (part.strip() for part in segment.split("=", 1))
    if key in fields:
        raise SystemExit("duplicate ExecStopPost field")
    fields[key] = field
expected = "/usr/lib/pinvou3/supervisor/pinvou-supervisor"
if fields.get("path") != expected or fields.get("argv[]", "").split() != [expected, "snapshot-app"]:
    raise SystemExit("app ExecStopPost is not the fixed snapshot client")
'
}

validate_generic_desktop() {
  [ ! -L "$GENERIC_DESKTOP" ] && [ -f "$GENERIC_DESKTOP" ] \
    || fail "generic desktop is missing or a symlink"
  [ "$(/usr/bin/stat -c %u:%g:%a "$GENERIC_DESKTOP")" = 0:0:644 ] \
    || fail "generic desktop owner/mode mismatch"
  [ "$(/usr/bin/grep -c '^Exec=/usr/bin/pinvou3-tauri$' "$GENERIC_DESKTOP")" -eq 1 ] \
    || fail "generic desktop no longer has the direct launch contract"
  ! /usr/bin/grep -q pinvou-supervisor "$GENERIC_DESKTOP" \
    || fail "generic desktop unexpectedly routes through Supervisor"
}

verify_common() {
  ensure_state_directory
  recover_all_staging_orphans || fail "cannot safely recover a prior E2E staging orphan"
  assert_no_e2e_assets
  load_and_verify_baseline
  [ -x "$PROFILE_HELPER" ] && [ -x "$SUPERVISOR" ] \
    || fail "installed fixed helpers are missing"
  [ "$(sha256_of /usr/share/pinvou3/supervisor/profiles/megabook-canary.conf)" \
    = "$PROFILE_SHA256" ] || fail "installed profile asset hash mismatch"
  [ "$(sha256_of /usr/share/pinvou3/supervisor/profiles/pinvou3-megabook-canary.desktop)" \
    = "$DESKTOP_SHA256" ] || fail "installed canary desktop asset hash mismatch"
  validate_generic_desktop
  [ "$(unit_property "$APP_UNIT" ActiveState)" = inactive ] \
    || fail "E2E requires the app to be exactly inactive before cleanup ownership begins"
  [ "$(unit_property "$APP_UNIT" MainPID)" = 0 ] \
    || fail "inactive app still has a MainPID"
  capture_initial_control_unit_states
  [ "$(unit_property "$ASR_UNIT" ActiveState)" = active ] \
    || fail "ASR is no longer active at the E2E cleanup boundary"
  initial_asr_pid=$(unit_property "$ASR_UNIT" MainPID)
  case "$initial_asr_pid" in
    ''|0|*[!0-9]*) fail "active ASR has no valid MainPID at the E2E cleanup boundary" ;;
  esac
  asr_was_active=1
  cleanup_enabled=1
  [ "$("$PROFILE_HELPER" status)" = inactive ] \
    || fail "E2E requires an initially inactive helper-owned profile"
  assert_no_transaction_residue
  profile_owned=1
  "$PROFILE_HELPER" activate >/dev/null
  [ "$("$PROFILE_HELPER" status)" = active ] || fail "profile activation did not reconcile"

  # postinst must not restart ASR. That invariant was checked above; this explicit restart is the
  # acceptance boundary that makes the newly installed ASR cgroup drop-in effective.
  /usr/bin/systemctl --user restart "$ASR_UNIT"
  wait_for_property "$ASR_UNIT" ActiveState active 30 || fail "ASR did not restart"
  [ "$(unit_property "$ASR_UNIT" InvocationID)" != "$(baseline_value asr_invocation_id)" ] \
    || fail "ASR restart did not create a new InvocationID"

  /usr/bin/systemctl --user start "$SOCKET_UNIT"
  wait_for_property "$SOCKET_UNIT" ActiveState active 10 || fail "Supervisor socket is inactive"
  assert_receipt pinvou_asr status "$SUPERVISOR" snapshot-asr
  assert_receipt pinvou_app status "$SUPERVISOR" status

  # The first side effect must traverse the installed hardened client, including reverse
  # SCM_CREDENTIALS validation. Python below only repeats that already-completed request id.
  recover_all_staging_orphans || fail "cannot safely recover E2E staging before Launch"
  assert_no_e2e_assets
  launch_offset=$(capture_append_offset "$control_ledger" 3145728)
  app_started=1
  "$SUPERVISOR" launch || fail "installed hardened launch client failed"
  wait_for_property "$APP_UNIT" ActiveState active 30 || fail "supervised app did not start"
  validate_effective_app_profile
  app_invocation=$(unit_property "$APP_UNIT" InvocationID)
  supervisor_pid=$(unit_property "$SUPERVISOR_UNIT" MainPID)
  case "$supervisor_pid" in ''|0|*[!0-9]*) fail "Supervisor has no live MainPID" ;; esac

  validate_user_file "$control_ledger" 600
  validate_user_file "$observation_journal" 600
  request_id=$(control_launch_record request_id "$launch_offset") \
    || fail "cannot identify the hardened launch ledger record"
  first_receipt=$(control_launch_record receipt "$launch_offset") \
    || fail "cannot recover the hardened launch terminal receipt"
  control_hash=$(sha256_of "$control_ledger")
  replay_receipt=$(send_fixed_launch "$request_id") || fail "same-id launch replay failed"
  [ "$replay_receipt" = "$first_receipt" ] \
    || fail "same-id replay did not return the hardened launch receipt"
  [ "$(sha256_of "$control_ledger")" = "$control_hash" ] \
    || fail "same-id replay mutated the durable control ledger"

  /usr/bin/systemctl --user restart "$SUPERVISOR_UNIT"
  wait_for_property "$SUPERVISOR_UNIT" ActiveState active 20 || fail "Supervisor did not restart"
  replay_receipt=$(send_fixed_launch "$request_id") || fail "durable launch replay failed"
  [ "$replay_receipt" = "$first_receipt" ] || fail "replayed request returned a different receipt"
  [ "$(sha256_of "$control_ledger")" = "$control_hash" ] \
    || fail "replayed request mutated the durable control ledger"
  [ "$(unit_property "$APP_UNIT" InvocationID)" = "$app_invocation" ] \
    || fail "replayed request launched a second app instance"

  # Same uid is insufficient for ASR control: only the live app MainPID is authorized. This raw
  # client is intentionally a negative server-side credential test and must not touch the ledger.
  asr_negative_generation=$(unit_property "$ASR_UNIT" InvocationID)
  asr_negative_pid=$(unit_property "$ASR_UNIT" MainPID)
  negative_hash=$(sha256_of "$control_ledger")
  send_same_uid_asr_stop_negative \
    "megabook-e2e-negative-$$-$(/usr/bin/stat -c %Y "$baseline_file")" \
    "$asr_negative_generation" || fail "same-UID ASR Stop negative test failed"
  [ "$(sha256_of "$control_ledger")" = "$negative_hash" ] \
    || fail "pre-authorization ASR rejection mutated the control ledger"
  [ "$(unit_property "$ASR_UNIT" ActiveState)" = active ] \
    || fail "negative ASR request changed ActiveState"
  [ "$(unit_property "$ASR_UNIT" InvocationID)" = "$asr_negative_generation" ] \
    || fail "negative ASR request changed InvocationID"
  [ "$(unit_property "$ASR_UNIT" MainPID)" = "$asr_negative_pid" ] \
    || fail "negative ASR request changed MainPID"
  assert_receipt pinvou_app status "$SUPERVISOR" status
  /usr/bin/printf '%s\n' \
    "safe-e2e-pass appInvocationID=$app_invocation supervisorMainPID=$(unit_property "$SUPERVISOR_UNIT" MainPID)"
}

validate_host_memory_gate() {
  /usr/bin/python3 -I - \
    /proc/meminfo /sys/fs/cgroup/cgroup.controllers /proc/self/cgroup /proc/swaps <<'PY'
import pathlib, sys

values = {}
for line in pathlib.Path(sys.argv[1]).read_text(encoding="ascii").splitlines():
    fields = line.split()
    if len(fields) >= 2 and fields[0].endswith(":"):
        values[fields[0][:-1]] = int(fields[1])
gib_kib = 1024 * 1024
total = values.get("MemTotal", 0)
available = values.get("MemAvailable", 0)
if not 30 * gib_kib <= total <= 34 * gib_kib:
    raise SystemExit("host is not the calibrated approximately-32-GiB MegaBook")
if available < 18 * gib_kib:
    raise SystemExit("MemAvailable is below the fixed 18-GiB safety gate")
if values.get("SwapTotal", -1) != 0 or values.get("SwapFree", -1) != 0:
    raise SystemExit("host swap must be disabled for the destructive memory E2E")
swap_lines = pathlib.Path(sys.argv[4]).read_text(encoding="ascii").splitlines()
if len(swap_lines) != 1 or not swap_lines[0].split() or swap_lines[0].split()[0] != "Filename":
    raise SystemExit("/proc/swaps reports an active swap device or malformed evidence")
controllers = pathlib.Path(sys.argv[2]).read_text(encoding="ascii").split()
if "memory" not in controllers:
    raise SystemExit("unified cgroup v2 memory controller is unavailable")
unified = [line for line in pathlib.Path(sys.argv[3]).read_text().splitlines() if line.startswith("0::")]
if len(unified) != 1:
    raise SystemExit("process is not running on a unified cgroup v2 hierarchy")
PY
}

ensure_runtime_parent() {
  directory=$1
  if [ ! -e "$directory" ]; then
    /usr/bin/mkdir -m 0700 -- "$directory" || fail "cannot create runtime parent: $directory"
  fi
  validate_owned_directory "$directory"
}

stage_memory_phase() {
  phase=$1
  validate_fixture_sources
  remove_e2e_assets || fail "cannot clear the previous fixed memory phase"
  /usr/bin/systemctl --user reset-failed "$APP_UNIT" >/dev/null 2>&1 || true
  validate_owned_directory "$runtime_dir"
  ensure_runtime_parent "$runtime_dir/systemd"
  ensure_runtime_parent "$runtime_unit_dir"
  ensure_runtime_directory "$e2e_runtime_dir" 700
  ensure_runtime_directory "$e2e_dropin_dir" 700
  stage_fixed_fixture "$loader_source" "$loader_target" "$e2e_runtime_dir" 700 "$LOADER_SHA256"
  case "$phase" in
    high)
      phase_dropin_source=$high_dropin_source
      phase_dropin_hash=$HIGH_DROPIN_SHA256
      ;;
    max)
      phase_dropin_source=$max_dropin_source
      phase_dropin_hash=$MAX_DROPIN_SHA256
      ;;
    *) fail "memory phase is not fixed" ;;
  esac
  stage_fixed_fixture \
    "$phase_dropin_source" "$e2e_dropin_target" "$e2e_dropin_dir" 644 "$phase_dropin_hash"
  /usr/bin/systemctl --user daemon-reload || fail "cannot load the fixed memory E2E drop-in"
  validate_fixed_file "$loader_target" "$uid" 700 "$LOADER_SHA256"
  validate_fixed_file "$e2e_dropin_target" "$uid" 644 "$phase_dropin_hash"
}

release_memory_loader() {
  phase=$1
  case "$phase" in
    high)
      go_source=$high_go_source
      go_target=$high_go_marker
      go_hash=$HIGH_GO_SHA256
      ;;
    max)
      go_source=$max_go_source
      go_target=$max_go_marker
      go_hash=$MAX_GO_SHA256
      ;;
    *) fail "loader release phase is not fixed" ;;
  esac
  stage_fixed_fixture "$go_source" "$go_target" "$e2e_runtime_dir" 600 "$go_hash"
}

wait_for_loader_ready() {
  phase=$1
  case "$phase" in
    high) evidence=$high_evidence ;;
    max) evidence=$max_evidence ;;
    *) fail "loader evidence phase is not fixed" ;;
  esac
  attempts=0
  while [ "$attempts" -lt 300 ]; do
    if [ -e "$evidence" ] || [ -L "$evidence" ]; then
      validate_evidence_file "$evidence" "$phase" && return 0
      return 1
    fi
    /usr/bin/sleep 0.1
    attempts=$((attempts + 1))
  done
  return 1
}

loader_evidence_pid() {
  phase=$1
  case "$phase" in
    high) evidence=$high_evidence ;;
    max) evidence=$max_evidence ;;
    *) fail "loader PID phase is not fixed" ;;
  esac
  /usr/bin/python3 -I - "$evidence" <<'PY'
import json, pathlib, sys
value = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
pid = value.get("pid")
if not isinstance(pid, int) or pid <= 1:
    raise SystemExit("loader evidence PID is invalid")
print(pid)
PY
}

proc_unified_cgroup() {
  pid=$1
  case "$pid" in ''|*[!0-9]*) fail "evidence PID is not numeric" ;; esac
  /usr/bin/awk -F: '$1 == "0" && $2 == "" { print $3; count += 1 } END { if (count != 1) exit 1 }' \
    "/proc/$pid/cgroup"
}

proc_starttime() {
  pid=$1
  case "$pid" in ''|*[!0-9]*) fail "evidence PID is not numeric" ;; esac
  /usr/bin/python3 -I - "$pid" <<'PY'
import pathlib, sys

pid = int(sys.argv[1])
raw = pathlib.Path(f"/proc/{pid}/stat").read_text(encoding="ascii")
prefix = f"{pid} ("
delimiter = raw.rfind(") ")
if not raw.startswith(prefix) or delimiter < len(prefix):
    raise SystemExit("/proc PID stat has an invalid comm boundary")
fields = raw[delimiter + 2:].split()
# The suffix starts at proc(5) field 3 (state); starttime is field 22, hence index 19.
if len(fields) <= 19 or not fields[19].isdigit() or int(fields[19]) <= 0:
    raise SystemExit("/proc PID stat starttime is invalid")
print(fields[19])
PY
}

pid_identity_retired() {
  pid=$1
  previous_starttime=$2
  [ ! -e "/proc/$pid" ] && return 0
  current_starttime=$(proc_starttime "$pid" 2>/dev/null) || {
    [ ! -e "/proc/$pid" ] && return 0
    return 1
  }
  [ "$current_starttime" != "$previous_starttime" ]
}

read_fixed_cgroup_value() {
  file=$1
  /usr/bin/python3 -I - "$file" <<'PY'
import pathlib, sys
path = pathlib.Path(sys.argv[1])
if path.is_symlink() or not path.is_file():
    raise SystemExit("fixed cgroup property is unavailable")
value = path.read_text(encoding="ascii").strip()
if "\n" in value or not value:
    raise SystemExit("fixed cgroup property is malformed")
print(value)
PY
}

find_webkit_in_app_cgroup() {
  expected_cgroup=$1
  /usr/bin/python3 -I - "$expected_cgroup" "$uid" <<'PY'
import os, pathlib, sys
expected = sys.argv[1]
uid = int(sys.argv[2])
matches = []
for proc in pathlib.Path("/proc").iterdir():
    if not proc.name.isdecimal():
        continue
    try:
        if proc.stat(follow_symlinks=False).st_uid != uid:
            continue
        cgroups = [
            fields[2]
            for line in (proc / "cgroup").read_text(encoding="ascii").splitlines()
            if len(fields := line.split(":", 2)) == 3 and fields[0] == "0" and fields[1] == ""
        ]
        if cgroups != [expected]:
            continue
        command = (proc / "cmdline").read_bytes().split(b"\0")
        if any(b"WebKitWebProcess" in argument for argument in command if argument):
            matches.append(int(proc.name))
    except (FileNotFoundError, PermissionError, ProcessLookupError, OSError, UnicodeError):
        continue
if not matches:
    raise SystemExit("no real WebKitWebProcess was observed in the app ControlGroup")
print(min(matches))
PY
}

wait_for_webkit_in_app_cgroup() {
  expected_cgroup=$1
  attempts=0
  while [ "$attempts" -lt 300 ]; do
    if webkit_candidate=$(find_webkit_in_app_cgroup "$expected_cgroup" 2>/dev/null); then
      [ "$(proc_unified_cgroup "$webkit_candidate" 2>/dev/null)" = "$expected_cgroup" ] \
        || return 1
      /usr/bin/printf '%s\n' "$webkit_candidate"
      return 0
    fi
    /usr/bin/sleep 0.1
    attempts=$((attempts + 1))
  done
  return 1
}

validate_phase_isolation() {
  phase=$1
  expected_supervisor_pid=$2
  expected_supervisor_invocation=$3
  validate_effective_app_profile
  app_cgroup=$(unit_property "$APP_UNIT" ControlGroup)
  case "$app_cgroup" in
    /*/pinvou3-app.service) ;;
    *) fail "app ControlGroup is not the fixed service cgroup" ;;
  esac
  app_cgroup_dir=/sys/fs/cgroup$app_cgroup
  [ "$(read_fixed_cgroup_value "$app_cgroup_dir/memory.high")" = 4294967296 ] \
    || fail "cgroup memory.high is not 4 GiB"
  [ "$(read_fixed_cgroup_value "$app_cgroup_dir/memory.max")" = 8589934592 ] \
    || fail "cgroup memory.max is not 8 GiB"
  [ "$(read_fixed_cgroup_value "$app_cgroup_dir/memory.swap.max")" = 2147483648 ] \
    || fail "cgroup memory.swap.max is not 2 GiB"
  [ "$(read_fixed_cgroup_value "$app_cgroup_dir/memory.oom.group")" = 1 ] \
    || fail "cgroup memory.oom.group is not enabled"
  if [ "$phase" = high ]; then
    memory_current=$(read_fixed_cgroup_value "$app_cgroup_dir/memory.current")
    case "$memory_current" in ''|*[!0-9]*) fail "cgroup memory.current is invalid" ;; esac
    [ "$memory_current" -le 2147483648 ] \
      || fail "High phase baseline exceeds the fixed 2-GiB cgroup headroom gate"
  fi

  case "$phase" in high) evidence=$high_evidence ;; max) evidence=$max_evidence ;; esac
  validate_evidence_file "$evidence" "$phase" "$app_cgroup"
  loader_pid=$(loader_evidence_pid "$phase")
  [ "$(/usr/bin/stat -c %u "/proc/$loader_pid")" = "$uid" ] \
    || fail "loader evidence PID owner mismatch"
  [ "$(proc_unified_cgroup "$loader_pid")" = "$app_cgroup" ] \
    || fail "loader is outside the app ControlGroup"
  app_main_pid=$(unit_property "$APP_UNIT" MainPID)
  [ "$(proc_unified_cgroup "$app_main_pid")" = "$app_cgroup" ] \
    || fail "app MainPID is outside its ControlGroup"
  webkit_pid=$(wait_for_webkit_in_app_cgroup "$app_cgroup") \
    || fail "the real WebKit child is not proven inside the app ControlGroup"
  [ "$(proc_unified_cgroup "$webkit_pid")" = "$app_cgroup" ] \
    || fail "WebKit cgroup changed during validation"

  [ "$(unit_property "$SUPERVISOR_UNIT" MainPID)" = "$expected_supervisor_pid" ] \
    || fail "Supervisor PID changed before loader release"
  [ "$(unit_property "$SUPERVISOR_UNIT" InvocationID)" = "$expected_supervisor_invocation" ] \
    || fail "Supervisor InvocationID changed before loader release"
  supervisor_cgroup=$(unit_property "$SUPERVISOR_UNIT" ControlGroup)
  case "$supervisor_cgroup" in
    "$app_cgroup"|"$app_cgroup"/*) fail "Supervisor is inside the app cgroup subtree" ;;
  esac
  [ "$(proc_unified_cgroup "$expected_supervisor_pid")" = "$supervisor_cgroup" ] \
    || fail "Supervisor MainPID does not match its ControlGroup"
}

runtime_baseline_ready() {
  offset=$1
  expected_generation=$2
  /usr/bin/python3 -I - "$runtime_ledger" "$offset" "$expected_generation" <<'PY'
import hashlib, json, pathlib, sys
path = pathlib.Path(sys.argv[1])
token = sys.argv[2]
generation = sys.argv[3]
raw = path.read_bytes()
offset_text, inode_text, prefix_hash = token.split(":")
offset, inode = int(offset_text), int(inode_text)
if inode and path.stat().st_ino != inode:
    raise SystemExit("Runtime ledger baseline inode changed")
if hashlib.sha256(raw[:offset]).hexdigest() != prefix_hash:
    raise SystemExit("Runtime ledger baseline prefix changed")
if offset > len(raw) or (offset and raw[offset - 1:offset] != b"\n"):
    raise SystemExit("Runtime ledger baseline offset changed")
for frame in raw[offset:].splitlines():
    envelope = json.loads(frame)
    event = envelope.get("event") or {}
    if event.get("kind") != "resource_observed":
        continue
    data = event.get("data") or {}
    observation = data.get("observation") or {}
    cgroup = observation.get("appCgroup") or {}
    if cgroup.get("instanceGeneration") != generation:
        continue
    if cgroup.get("memoryHighBytes") != 4 * 1024**3 or cgroup.get("memoryMaxBytes") != 8 * 1024**3:
        continue
    current = cgroup.get("memoryCurrentBytes")
    if not isinstance(current, int) or current >= 4 * 1024**3:
        continue
    if data.get("pressure") == "critical":
        continue
    raise SystemExit(0)
raise SystemExit("trusted below-high app cgroup baseline not yet present")
PY
}

wait_for_runtime_baseline() {
  offset=$1
  expected_generation=$2
  attempts=0
  while [ "$attempts" -lt 60 ]; do
    if runtime_baseline_ready "$offset" "$expected_generation" >/dev/null 2>&1; then
      return 0
    fi
    /usr/bin/sleep 1
    attempts=$((attempts + 1))
  done
  return 1
}

runtime_baseline_high_counter() {
  token=$1
  expected_generation=$2
  /usr/bin/python3 -I - "$runtime_ledger" "$token" "$expected_generation" <<'PY'
import hashlib, json, pathlib, sys
path = pathlib.Path(sys.argv[1])
token = sys.argv[2]
generation = sys.argv[3]
raw = path.read_bytes()
offset_text, inode_text, prefix_hash = token.split(":")
offset, inode = int(offset_text), int(inode_text)
if inode and path.stat().st_ino != inode:
    raise SystemExit("Runtime ledger baseline inode changed")
if hashlib.sha256(raw[:offset]).hexdigest() != prefix_hash:
    raise SystemExit("Runtime ledger baseline prefix changed")
candidates = []
for frame in raw[offset:].splitlines():
    envelope = json.loads(frame)
    event = envelope.get("event") or {}
    if event.get("kind") != "resource_observed":
        continue
    data = event.get("data") or {}
    cgroup = ((data.get("observation") or {}).get("appCgroup") or {})
    if cgroup.get("instanceGeneration") != generation:
        continue
    current = cgroup.get("memoryCurrentBytes")
    high = cgroup.get("memoryEventsHigh")
    if not isinstance(current, int) or current >= 4 * 1024**3 or not isinstance(high, int):
        continue
    candidates.append((envelope.get("sequence", -1), high))
if not candidates:
    raise SystemExit("trusted below-high counter baseline is absent")
print(max(candidates)[1])
PY
}

app_host_work_current_state() {
  expected_state=$1
  /usr/bin/python3 -I - "$runtime_ledger" "$expected_state" <<'PY'
import json, pathlib, sys

path = pathlib.Path(sys.argv[1])
expected_state = sys.argv[2]
raw = path.read_bytes()
if not raw or raw[-1:] != b"\n" or len(raw) > 64 * 1024 * 1024:
    raise SystemExit("Runtime ledger is not a bounded committed snapshot")
identity = None
state = None
for frame in raw.splitlines():
    envelope = json.loads(frame)
    event = envelope.get("event") or {}
    kind = event.get("kind")
    data = event.get("data") or {}
    if kind == "host_work_registered":
        work = data.get("work") or {}
        if work.get("owner") == "host:supervisor-app" and work.get("kind") == "app_cgroup":
            work_id = work.get("workId")
            generation = work.get("generation")
            if not isinstance(work_id, str) or not isinstance(generation, int) \
                or isinstance(generation, bool) or generation <= 0:
                raise SystemExit("App HostWork registration identity is invalid")
            identity = (work_id, generation)
            state = work.get("observedState")
    elif kind == "host_work_observed" and identity is not None:
        if (data.get("work_id"), data.get("generation")) == identity:
            state = data.get("observed_state")
    elif kind == "host_work_unregistered" and identity is not None:
        if (data.get("work_id"), data.get("generation")) == identity:
            identity = None
            state = None
if identity is None or state != expected_state:
    raise SystemExit("App HostWork projection has not reached the expected state")
PY
}

wait_for_app_host_work_state() {
  expected_state=$1
  attempts=$2
  index=0
  while [ "$index" -lt "$attempts" ]; do
    if app_host_work_current_state "$expected_state" >/dev/null 2>&1; then
      return 0
    fi
    /usr/bin/sleep 1
    index=$((index + 1))
  done
  return 1
}

app_host_work_stopped_after() {
  token=$1
  /usr/bin/python3 -I - "$runtime_ledger" "$token" <<'PY'
import hashlib, json, pathlib, sys

path = pathlib.Path(sys.argv[1])
token = sys.argv[2]
raw = path.read_bytes()
parts = token.split(":")
if len(parts) != 3:
    raise SystemExit("Runtime ledger append token is invalid")
offset, inode, prefix_hash = int(parts[0]), int(parts[1]), parts[2]
if inode and path.stat().st_ino != inode:
    raise SystemExit("Runtime ledger inode changed")
if offset < 0 or offset > len(raw) or hashlib.sha256(raw[:offset]).hexdigest() != prefix_hash:
    raise SystemExit("Runtime ledger prefix changed")
if offset and raw[offset - 1:offset] != b"\n":
    raise SystemExit("Runtime ledger append boundary changed")
if len(raw) - offset > 1024 * 1024:
    raise SystemExit("Runtime ledger stop-observation tail exceeded its bound")

registrations = {}
for frame in raw.splitlines():
    envelope = json.loads(frame)
    event = envelope.get("event") or {}
    if event.get("kind") != "host_work_registered":
        continue
    work = (event.get("data") or {}).get("work") or {}
    work_id = work.get("workId")
    generation = work.get("generation")
    if work.get("owner") == "host:supervisor-app" and work.get("kind") == "app_cgroup" \
        and isinstance(work_id, str) and isinstance(generation, int) \
        and not isinstance(generation, bool) and generation > 0:
        registrations[(work_id, generation)] = work

for frame in raw[offset:].splitlines():
    envelope = json.loads(frame)
    event = envelope.get("event") or {}
    if event.get("kind") != "host_work_observed":
        continue
    data = event.get("data") or {}
    identity = (data.get("work_id"), data.get("generation"))
    if identity not in registrations or data.get("observed_state") != "stopped":
        continue
    if envelope.get("schemaVersion") != 6 \
        or envelope.get("sourceActorId") != "adapter:host-work" \
        or envelope.get("correlationId") != f"host-work:{identity[0]}":
        continue
    raise SystemExit(0)
raise SystemExit("current App HostWork stop observation is absent")
PY
}

wait_for_app_host_work_stopped_after() {
  token=$1
  attempts=0
  while [ "$attempts" -lt 30 ]; do
    if app_host_work_stopped_after "$token" >/dev/null 2>&1; then
      return 0
    fi
    /usr/bin/sleep 1
    attempts=$((attempts + 1))
  done
  return 1
}

wait_for_asr_stop() {
  attempts=0
  while [ "$attempts" -lt 120 ]; do
    if [ "$(unit_property "$ASR_UNIT" ActiveState 2>/dev/null)" = inactive ]; then
      return 0
    fi
    /usr/bin/sleep 1
    attempts=$((attempts + 1))
  done
  return 1
}

wait_for_stable_asr_stop() {
  attempts=0
  while [ "$attempts" -lt 5 ]; do
    [ "$(unit_property "$ASR_UNIT" ActiveState 2>/dev/null)" = inactive ] || return 1
    [ "$(unit_property "$ASR_UNIT" MainPID 2>/dev/null)" = 0 ] || return 1
    # systemd clears InvocationID when this inactive runtime cycle has fully retired. The old
    # 32-hex identity remains attributable in Runtime + Supervisor Pending/before evidence.
    [ -z "$(unit_property "$ASR_UNIT" InvocationID 2>/dev/null)" ] || return 1
    /usr/bin/sleep 1
    attempts=$((attempts + 1))
  done
  return 0
}

wait_for_app_generation_change() {
  previous=$1
  attempts=0
  while [ "$attempts" -lt 180 ]; do
    state=$(unit_property "$APP_UNIT" ActiveState 2>/dev/null || true)
    generation=$(unit_property "$APP_UNIT" InvocationID 2>/dev/null || true)
    if [ "$state" != active ] || [ "$generation" != "$previous" ]; then
      return 0
    fi
    /usr/bin/sleep 1
    attempts=$((attempts + 1))
  done
  return 1
}

verify_high_evidence() {
  runtime_offset=$1
  control_offset=$2
  observation_offset=$3
  baseline_high=$4
  app_generation=$5
  asr_generation=$6
  /usr/bin/python3 -I - \
    "$runtime_ledger" "$runtime_offset" "$control_ledger" "$control_offset" \
    "$observation_journal" "$observation_offset" "$app_generation" "$asr_generation" \
    "$baseline_high" <<'PY'
import hashlib, json, pathlib, sys

runtime_path = pathlib.Path(sys.argv[1])
runtime_token = sys.argv[2]
control_path = pathlib.Path(sys.argv[3])
control_token = sys.argv[4]
observation_path = pathlib.Path(sys.argv[5])
observation_token = sys.argv[6]
app_generation = sys.argv[7]
asr_generation = sys.argv[8]
baseline_high = int(sys.argv[9])

def verify_prefix(path, raw, token, label):
    offset_text, inode_text, prefix_hash = token.split(":")
    offset, inode = int(offset_text), int(inode_text)
    if inode and path.stat().st_ino != inode:
        raise SystemExit(f"{label} inode changed")
    if offset < 0 or offset > len(raw):
        raise SystemExit(f"{label} offset changed")
    if hashlib.sha256(raw[:offset]).hexdigest() != prefix_hash:
        raise SystemExit(f"{label} prefix changed")
    if offset and raw[offset - 1:offset] != b"\n":
        raise SystemExit(f"{label} window is not on a committed boundary")
    return offset

def require_envelope(envelope, source, correlation):
    return (
        envelope.get("schemaVersion") == 6
        and envelope.get("sourceActorId") == source
        and envelope.get("correlationId") == correlation
        and isinstance(envelope.get("sequence"), int)
        and isinstance(envelope.get("eventId"), str)
    )

runtime_raw = runtime_path.read_bytes()
runtime_offset = verify_prefix(runtime_path, runtime_raw, runtime_token, "Runtime ledger")
if len(runtime_raw) - runtime_offset > 1024 * 1024:
    raise SystemExit("Runtime ledger E2E tail exceeded its bound")
runtime_all = [json.loads(frame) for frame in runtime_raw.splitlines()]
runtime_tail = [json.loads(frame) for frame in runtime_raw[runtime_offset:].splitlines()]
owners = {}
for envelope in runtime_all:
    event = envelope.get("event") or {}
    if event.get("kind") == "host_work_registered":
        work = (event.get("data") or {}).get("work") or {}
        if envelope.get("schemaVersion") != 6 \
            or envelope.get("sourceActorId") != "kernel:pinvou-os" \
            or envelope.get("correlationId") is not None \
            or not isinstance(envelope.get("sequence"), int) \
            or not isinstance(envelope.get("eventId"), str):
            raise SystemExit("HostWork registration envelope is invalid")
        work_id = work.get("workId")
        work_generation = work.get("generation")
        if not isinstance(work_id, str) or not isinstance(work_generation, int) \
            or isinstance(work_generation, bool) or work_generation <= 0:
            raise SystemExit("HostWork registration identity/generation is invalid")
        owners[work_id] = (work.get("owner"), work.get("kind"), work_generation)
    elif event.get("kind") == "host_work_unregistered":
        data = event.get("data") or {}
        work_id = data.get("work_id")
        generation = data.get("generation")
        if owners.get(work_id, (None, None, None))[2] == generation:
            owners.pop(work_id)

app_registrations = []
for envelope in runtime_tail:
    event = envelope.get("event") or {}
    if event.get("kind") != "host_work_registered":
        continue
    work = (event.get("data") or {}).get("work") or {}
    if work.get("owner") == "host:supervisor-app" and work.get("kind") == "app_cgroup":
        if envelope.get("schemaVersion") != 6 \
            or envelope.get("sourceActorId") != "kernel:pinvou-os" \
            or envelope.get("correlationId") is not None:
            raise SystemExit("current App HostWork registration envelope is invalid")
        if work.get("essential") is True \
            and work.get("governable") is False \
            and work.get("supportedActions") == []:
            app_registrations.append(work)
if not app_registrations:
    raise SystemExit("current High window lacks essential/non-governable App HostWork registration")
app_identities = {
    (work.get("workId"), work.get("generation")) for work in app_registrations
    if isinstance(work.get("workId"), str)
    and isinstance(work.get("generation"), int)
    and not isinstance(work.get("generation"), bool)
    and work.get("generation") > 0
}
if len(app_identities) != len(app_registrations):
    raise SystemExit("current App HostWork registration identity/generation is invalid")
for envelope in runtime_tail:
    event = envelope.get("event") or {}
    if event.get("kind") == "host_work_directive_issued":
        directive = (event.get("data") or {}).get("directive") or {}
        if (directive.get("workId"), directive.get("generation")) in app_identities:
            raise SystemExit("current High window attempted to govern the app HostWork")

criticals = []
for envelope in runtime_tail:
    event = envelope.get("event") or {}
    if event.get("kind") != "resource_observed":
        continue
    if not require_envelope(envelope, "agent:resource", "resource-pressure"):
        continue
    data = event.get("data") or {}
    cgroup = ((data.get("observation") or {}).get("appCgroup") or {})
    if data.get("pressure") != "critical" or cgroup.get("instanceGeneration") != app_generation:
        continue
    if cgroup.get("memoryHighBytes") != 4 * 1024**3 or cgroup.get("memoryMaxBytes") != 8 * 1024**3:
        continue
    current = cgroup.get("memoryCurrentBytes")
    high_events = cgroup.get("memoryEventsHigh")
    if (not isinstance(current, int) or current < 4 * 1024**3) and (
        not isinstance(high_events, int) or high_events <= baseline_high
    ):
        continue
    if not isinstance(cgroup.get("observedAtMs"), int) or not isinstance(high_events, int):
        continue
    criticals.append((envelope, cgroup))
if not criticals:
    raise SystemExit("no current-round ResourceObserved Critical cgroup evidence")

chains = []
for envelope in runtime_tail:
    event = envelope.get("event") or {}
    if event.get("kind") != "host_work_directive_issued":
        continue
    if not require_envelope(envelope, "kernel:resource-governor", "resource-pressure"):
        continue
    directive = (event.get("data") or {}).get("directive") or {}
    issued_sequence = envelope.get("sequence")
    work_id = directive.get("workId")
    generation = directive.get("generation")
    work_identity = owners.get(work_id)
    if work_identity != ("host:supervisor-asr", "asr_cgroup", generation):
        continue
    if directive.get("action") != "stop" or directive.get("policyRevision") != "resource-governor:v1":
        continue
    if directive.get("status") != "pending" \
        or not isinstance(directive.get("resourcePressureEpoch"), int) \
        or directive.get("resourcePressureEpoch", 0) <= 0:
        continue
    if any(key in directive for key in (
        "dispatchRecordedAtMs",
        "acknowledgement", "acknowledgedAtMs", "acknowledgementDetail",
        "reconciliation", "reconciledObservedState", "reconciledAtMs", "reconciliationDetail",
    )):
        continue
    causal = []
    for critical_envelope, critical_cgroup in criticals:
        if critical_envelope["sequence"] >= issued_sequence:
            continue
        for claim_envelope in runtime_tail:
            claim_sequence = claim_envelope.get("sequence", -1)
            if not critical_envelope["sequence"] < claim_sequence < issued_sequence:
                continue
            claim_event = claim_envelope.get("event") or {}
            if claim_event.get("kind") != "claim_asserted":
                continue
            if not require_envelope(claim_envelope, "agent:resource", "resource-pressure"):
                continue
            if claim_envelope.get("causationId") != critical_envelope["eventId"]:
                continue
            claim = (claim_event.get("data") or {}).get("claim") or {}
            if claim.get("assertedByActorId") != "agent:resource" \
                or claim.get("predicate") != "pressure_level" \
                or claim.get("evidenceEventIds") != [critical_envelope["eventId"]]:
                continue
            if (claim.get("value") or {}).get("level") != "critical" \
                or claim.get("active") is not True:
                continue
            if envelope.get("causationId") != claim_envelope["eventId"]:
                continue
            causal.append((critical_envelope, critical_cgroup, claim_envelope))
    if len(causal) != 1:
        continue
    directive_id = directive.get("directiveId")
    dispatches = []
    acknowledgements = []
    reconciliations = []
    for later in runtime_tail:
        if later.get("sequence", -1) <= issued_sequence:
            continue
        later_event = later.get("event") or {}
        later_data = later_event.get("data") or {}
        if later_event.get("kind") == "host_work_directive_dispatch_recorded" \
            and require_envelope(later, "adapter:host-work", f"host-work:{work_id}") \
            and later.get("causationId") == directive_id \
            and later_data.get("directive_id") == directive_id \
            and later_data.get("work_id") == work_id \
            and later_data.get("generation") == generation \
            and isinstance(later_data.get("dispatched_at_ms"), int):
            dispatches.append((later.get("sequence"), later_data.get("dispatched_at_ms")))
        if later_event.get("kind") == "host_work_directive_acknowledged" \
            and require_envelope(later, "adapter:host-work", f"host-work:{work_id}") \
            and later.get("causationId") == directive_id \
            and later_data.get("directive_id") == directive_id \
            and later_data.get("work_id") == work_id \
            and later_data.get("generation") == generation \
            and later_data.get("acknowledgement") == "applied" \
            and isinstance(later_data.get("acknowledged_at_ms"), int):
            acknowledgements.append((later.get("sequence"), later_data.get("acknowledged_at_ms")))
        if later_event.get("kind") == "host_work_directive_reconciled" \
            and require_envelope(later, "adapter:host-work", f"host-work:{work_id}") \
            and later.get("causationId") == directive_id \
            and later_data.get("directive_id") == directive_id \
            and later_data.get("work_id") == work_id \
            and later_data.get("generation") == generation \
            and later_data.get("outcome") == "confirmed" \
            and later_data.get("observed_state") == "stopped" \
            and isinstance(later_data.get("reconciled_at_ms"), int):
            reconciliations.append((later.get("sequence"), later_data.get("reconciled_at_ms")))
    if len(dispatches) == 1 and len(acknowledgements) == 1 and len(reconciliations) == 1:
        dispatch_sequence, dispatched_at = dispatches[0]
        ack_sequence, acknowledged_at = acknowledgements[0]
        reconcile_sequence, reconciled_at = reconciliations[0]
        if dispatch_sequence < ack_sequence < reconcile_sequence \
            and dispatched_at <= acknowledged_at <= reconciled_at:
            chains.append((directive_id, causal[0][1], dispatched_at, acknowledged_at))
if len(chains) != 1:
    raise SystemExit("current Runtime window does not contain one exact ASR Stop dispatch chain")
directive_id, critical_cgroup, dispatched_at, acknowledged_at = chains[0]

observation_raw = observation_path.read_bytes()
observation_offset = verify_prefix(
    observation_path, observation_raw, observation_token, "Supervisor observation journal"
)
if len(observation_raw) - observation_offset > 512 * 1024:
    raise SystemExit("Supervisor observation E2E tail exceeded its bound")
matching_observations = []
for frame in observation_raw[observation_offset:].splitlines():
    entry = json.loads(frame)
    if entry.get("event") != "observation" or entry.get("target") != "pinvou_app":
        continue
    if entry.get("descriptor_revision") != "pinvou-app-descriptor-v1":
        continue
    observation = entry.get("observation") or {}
    cgroup = observation.get("cgroup") or {}
    events = cgroup.get("memory_events") or {}
    if observation.get("instance_generation") != app_generation:
        continue
    if entry.get("integrity_error") is not None or entry.get("control_group_present") is not True:
        continue
    if cgroup.get("memory_high_bytes") != 4 * 1024**3 \
        or cgroup.get("memory_max_bytes") != 8 * 1024**3 \
        or cgroup.get("memory_swap_max_bytes") != 2 * 1024**3:
        continue
    if events.get("high") != critical_cgroup.get("memoryEventsHigh") \
        or events.get("oom") != critical_cgroup.get("memoryEventsOom") \
        or events.get("oom_kill") != critical_cgroup.get("memoryEventsOomKill"):
        continue
    if cgroup.get("memory_current_bytes") != critical_cgroup.get("memoryCurrentBytes"):
        continue
    recorded_at = entry.get("recorded_at_unix_ms")
    observed_at = critical_cgroup.get("observedAtMs")
    if not isinstance(recorded_at, int) or not isinstance(observed_at, int) \
        or abs(recorded_at - observed_at) > 2000:
        continue
    matching_observations.append(entry)
if not matching_observations:
    raise SystemExit("High Runtime fact has no matching trusted Supervisor app observation")

control_raw = control_path.read_bytes()
control_offset = verify_prefix(control_path, control_raw, control_token, "Supervisor control ledger")
if len(control_raw) - control_offset > 512 * 1024:
    raise SystemExit("Supervisor control E2E tail exceeded its bound")
control_tail = [json.loads(frame) for frame in control_raw[control_offset:].splitlines()]
pending = []
completed = []
for index, event in enumerate(control_tail):
    fingerprint = event.get("fingerprint") or {}
    if fingerprint.get("request_id") != directive_id:
        continue
    expected = {
        "request_id": directive_id,
        "target": "pinvou_asr",
        "descriptor_revision": "pinvou-asr-descriptor-v1",
        "expected_instance_generation": asr_generation,
        "action": "stop",
    }
    if fingerprint != expected:
        raise SystemExit("ASR control fingerprint does not match the Runtime directive")
    if event.get("event") == "control_pending":
        before = event.get("before") or {}
        if before.get("instance_generation") != asr_generation or before.get("state") != "active" \
            or not isinstance(before.get("main_pid"), int) or before.get("main_pid") <= 1:
            raise SystemExit("ASR Pending did not bind the live baseline generation")
        pending.append((index, event))
    elif event.get("event") == "control_completed_tombstone":
        receipt = event.get("receipt") or {}
        if any(receipt.get(key) != value for key, value in expected.items()):
            raise SystemExit("ASR terminal receipt does not match its Pending fingerprint")
        if receipt.get("outcome") != "applied":
            raise SystemExit("ASR terminal receipt did not confirm Stop")
        if receipt.get("protocol_version") != 2 \
            or not isinstance(receipt.get("observed_at_unix_ms"), int):
            raise SystemExit("ASR terminal receipt protocol/timestamp is invalid")
        observation = receipt.get("observation") or {}
        if observation.get("state") != "inactive" or observation.get("main_pid") is not None:
            raise SystemExit("ASR terminal receipt did not observe stopped state")
        completed.append((index, event))
if len(pending) != 1 or len(completed) != 1:
    raise SystemExit("ASR control ledger lacks one Pending plus one terminal tombstone")
pending_index, pending_event = pending[0]
completed_index, completed_event = completed[0]
if pending_index >= completed_index:
    raise SystemExit("ASR control terminal precedes Pending")
pending_at = pending_event.get("recorded_at_unix_ms")
completed_at = completed_event.get("recorded_at_unix_ms")
if not isinstance(pending_at, int) or not isinstance(completed_at, int) or pending_at > completed_at:
    raise SystemExit("ASR control timestamps are not monotonic")
if pending_at < dispatched_at or completed_at > acknowledged_at:
    raise SystemExit("ASR control ledger is outside the durable Runtime dispatch/ACK interval")
receipt_observed_at = (completed_event.get("receipt") or {}).get("observed_at_unix_ms")
if receipt_observed_at < pending_at or receipt_observed_at > completed_at:
    raise SystemExit("ASR receipt observation timestamp is outside its Pending/terminal interval")
print(directive_id)
PY
}

wait_for_high_evidence() {
  runtime_offset=$1
  control_offset=$2
  observation_offset=$3
  baseline_high=$4
  app_generation=$5
  asr_generation=$6
  attempts=0
  while [ "$attempts" -lt 90 ]; do
    if verify_high_evidence \
      "$runtime_offset" "$control_offset" "$observation_offset" "$baseline_high" \
      "$app_generation" "$asr_generation" \
      >/dev/null 2>&1; then
      verify_high_evidence \
        "$runtime_offset" "$control_offset" "$observation_offset" "$baseline_high" \
        "$app_generation" "$asr_generation"
      return 0
    fi
    /usr/bin/sleep 1
    attempts=$((attempts + 1))
  done
  return 1
}

snapshot_app_baseline() {
  "$SUPERVISOR" snapshot-app | /usr/bin/python3 -I -c '
import json, sys
r = json.load(sys.stdin)
if r.get("protocol_version") != 2 \
    or not str(r.get("request_id", "")).startswith("snapshot-app:") \
    or r.get("target") != "pinvou_app" \
    or r.get("descriptor_revision") != "pinvou-app-descriptor-v1" \
    or r.get("expected_instance_generation") is not None \
    or r.get("action") != "status" or r.get("outcome") != "reconciled":
    raise SystemExit("snapshot-app did not return a trusted status receipt")
o = r.get("observation") or {}
generation = o.get("instance_generation")
cgroup = o.get("cgroup") or {}
events = cgroup.get("memory_events") or {}
if not isinstance(generation, str) or len(generation) != 32:
    raise SystemExit("snapshot-app generation is invalid")
if o.get("state") != "active" or not isinstance(o.get("main_pid"), int) \
    or o.get("main_pid") <= 1 or not isinstance(o.get("restart_count"), int):
    raise SystemExit("snapshot-app did not bind the live old app generation")
if cgroup.get("memory_high_bytes") != 4 * 1024**3 \
    or cgroup.get("memory_max_bytes") != 8 * 1024**3 \
    or cgroup.get("memory_swap_max_bytes") != 2 * 1024**3:
    raise SystemExit("snapshot-app cgroup policy is not 4/8/2 GiB")
counters = [events.get(key, 0) for key in ("high", "max", "oom", "oom_kill", "oom_group_kill")]
if not all(isinstance(value, int) and value >= 0 for value in counters):
    raise SystemExit("snapshot-app memory.events baseline is invalid")
print(generation, o["restart_count"], *counters)
'
}

capture_app_journal_cursor() {
  cursor=$(/usr/bin/journalctl --user -u "$APP_UNIT" -n 1 --show-cursor --no-pager \
    | /usr/bin/awk '$1 == "--" && $2 == "cursor:" { print $3; count += 1 } END { if (count != 1) exit 1 }') \
    || fail "cannot capture the app journal cursor"
  /usr/bin/printf '%s' "$cursor" | /usr/bin/python3 -I -c '
import re, sys
value = sys.stdin.read()
if not re.fullmatch(r"[A-Za-z0-9_.:;=-]+", value):
    raise SystemExit("app journal cursor has an unsafe shape")
' || fail "app journal cursor has an unsafe shape"
  /usr/bin/printf '%s\n' "$cursor"
}

verify_oom_evidence() {
  token=$1
  journal_cursor=$2
  old_generation=$3
  high_before=$4
  max_before=$5
  oom_before=$6
  oom_kill_before=$7
  oom_group_kill_before=$8
  restart_count_before=$9
  fixture_tmp=$(/usr/bin/mktemp "$e2e_runtime_dir/.pinvou-e2e.XXXXXX") \
    || fail "cannot stage the bounded app journal tail"
  if ! /usr/bin/journalctl --user -u "$APP_UNIT" --after-cursor="$journal_cursor" \
    --output=json --no-pager >"$fixture_tmp"; then
    /usr/bin/rm -f -- "$fixture_tmp"
    fixture_tmp=
    return 1
  fi
  validate_user_file "$fixture_tmp" 600
  journal_bytes=$(/usr/bin/stat -c %s "$fixture_tmp") || return 1
  if [ "$journal_bytes" -gt 524288 ]; then
    /usr/bin/rm -f -- "$fixture_tmp"
    fixture_tmp=
    return 1
  fi
  if ! /usr/bin/python3 -I - \
      "$observation_journal" "$token" "$uid" "$old_generation" \
      "$high_before" "$max_before" "$oom_before" "$oom_kill_before" \
      "$oom_group_kill_before" "$restart_count_before" "$fixture_tmp" <<'PY'
import hashlib, json, pathlib, sys
path = pathlib.Path(sys.argv[1])
token = sys.argv[2]
uid = sys.argv[3]
generation = sys.argv[4]
before_keys = ("high", "max", "oom", "oom_kill", "oom_group_kill")
before = {key: int(value) for key, value in zip(before_keys, sys.argv[5:10])}
restart_count_before = int(sys.argv[10])
journal_path = pathlib.Path(sys.argv[11])
raw = path.read_bytes()
offset_text, inode_text, prefix_hash = token.split(":")
offset, inode = int(offset_text), int(inode_text)
if inode and path.stat().st_ino != inode:
    raise SystemExit("observation journal rotated during the OOM phase")
if offset < 0 or offset > len(raw) or hashlib.sha256(raw[:offset]).hexdigest() != prefix_hash:
    raise SystemExit("observation journal prefix changed during the OOM phase")
if offset and raw[offset - 1:offset] != b"\n":
    raise SystemExit("observation journal token is not a committed boundary")
if len(raw) - offset > 512 * 1024:
    raise SystemExit("observation journal OOM tail exceeded its bound")

observations = []
for frame in raw[offset:].splitlines():
    event = json.loads(frame)
    if event.get("event") != "observation" or event.get("target") != "pinvou_app" \
        or event.get("descriptor_revision") != "pinvou-app-descriptor-v1":
        continue
    observation = event.get("observation") or {}
    cgroup = observation.get("cgroup") or {}
    events = cgroup.get("memory_events") or {}
    if observation.get("instance_generation") != generation \
        or observation.get("unit_result") != "oom-kill" \
        or observation.get("state") != "deactivating" \
        or observation.get("main_pid") is not None:
        continue
    if event.get("control_group_present") is not True or event.get("integrity_error") is not None:
        continue
    if cgroup.get("memory_high_bytes") != 4 * 1024**3 \
        or cgroup.get("memory_max_bytes") != 8 * 1024**3 \
        or cgroup.get("memory_swap_max_bytes") != 2 * 1024**3:
        continue
    if not all(isinstance(events.get(key), int) and events[key] > before[key] for key in before_keys):
        continue
    if observation.get("restart_count") != restart_count_before:
        continue
    observations.append(event)
if not observations:
    raise SystemExit("Supervisor observation lacks old-generation 4/8/2 OOM counter deltas")

journal_receipts = []
for line in journal_path.read_text(encoding="utf-8").splitlines():
    entry = json.loads(line)
    if entry.get("_UID") != uid or entry.get("_SYSTEMD_USER_UNIT") != "pinvou3-app.service" \
        or entry.get("SYSLOG_IDENTIFIER") != "pinvou3-app":
        continue
    if entry.get("_SYSTEMD_INVOCATION_ID") != generation:
        continue
    message = entry.get("MESSAGE")
    if not isinstance(message, str):
        continue
    try:
        receipt = json.loads(message)
    except json.JSONDecodeError:
        continue
    if not str(receipt.get("request_id", "")).startswith("snapshot-app:"):
        continue
    if receipt.get("protocol_version") != 2 \
        or receipt.get("target") != "pinvou_app" \
        or receipt.get("descriptor_revision") != "pinvou-app-descriptor-v1" \
        or receipt.get("expected_instance_generation") is not None \
        or receipt.get("action") != "status" \
        or receipt.get("outcome") != "reconciled":
        continue
    observation = receipt.get("observation") or {}
    if observation.get("instance_generation") != generation \
        or observation.get("unit_result") != "oom-kill" \
        or observation.get("state") != "deactivating" \
        or observation.get("main_pid") is not None \
        or observation.get("restart_count") != restart_count_before:
        continue
    events = ((observation.get("cgroup") or {}).get("memory_events") or {})
    if not all(isinstance(events.get(key), int) and events[key] > before[key] for key in before_keys):
        continue
    if not any(observation == recorded.get("observation") for recorded in observations):
        continue
    journal_receipts.append(receipt)
if len(journal_receipts) != 1:
    raise SystemExit("journald does not contain one old-Invocation ExecStopPost snapshot receipt")
PY
  then
    /usr/bin/rm -f -- "$fixture_tmp"
    fixture_tmp=
    return 1
  fi
  /usr/bin/rm -- "$fixture_tmp" || return 1
  fixture_tmp=
  return 0
}

wait_for_oom_evidence() {
  attempts=0
  while [ "$attempts" -lt 180 ]; do
    if verify_oom_evidence "$@" >/dev/null 2>&1; then
      return 0
    fi
    /usr/bin/sleep 1
    attempts=$((attempts + 1))
  done
  return 1
}

wait_for_stable_restarted_app() {
  expected_generation=$1
  expected_restarts=$2
  attempts=0
  while [ "$attempts" -lt 30 ]; do
    [ "$(unit_property "$APP_UNIT" ActiveState 2>/dev/null)" = active ] || return 1
    [ "$(unit_property "$APP_UNIT" InvocationID 2>/dev/null)" = "$expected_generation" ] || return 1
    [ "$(unit_property "$APP_UNIT" NRestarts 2>/dev/null)" = "$expected_restarts" ] || return 1
    /usr/bin/sleep 1
    attempts=$((attempts + 1))
  done
  return 0
}

verify_memory_max() {
  validate_host_memory_gate
  verify_common

  # High and Max are deliberately separate app generations. The runtime fixture never receives a
  # PID, unit, cgroup path, property, or command from the operator.
  wait_for_app_host_work_state running 30 \
    || fail "App HostWork did not reach Running before the High phase boundary"
  high_runtime_offset=$(capture_append_offset "$runtime_ledger" 67108864)
  fixed_stop_app || fail "cannot stop the safe-stage app before the High phase"
  wait_for_app_host_work_stopped_after "$high_runtime_offset" \
    || fail "Resource control did not record the pre-High App HostWork Stop boundary"
  app_started=0
  stage_memory_phase high
  high_supervisor_pid=$(unit_property "$SUPERVISOR_UNIT" MainPID)
  high_supervisor_invocation=$(unit_property "$SUPERVISOR_UNIT" InvocationID)
  high_asr_generation=$(unit_property "$ASR_UNIT" InvocationID)
  app_started=1
  "$SUPERVISOR" launch || fail "hardened client could not launch the High phase"
  wait_for_property "$APP_UNIT" ActiveState active 30 || fail "High phase app did not start"
  high_app_generation=$(unit_property "$APP_UNIT" InvocationID)
  wait_for_loader_ready high || fail "High loader did not reach its fixed ready gate"
  validate_phase_isolation high "$high_supervisor_pid" "$high_supervisor_invocation"
  wait_for_runtime_baseline "$high_runtime_offset" "$high_app_generation" \
    || fail "High phase never established a trusted below-high Resource baseline"
  high_baseline_high=$(runtime_baseline_high_counter "$high_runtime_offset" "$high_app_generation") \
    || fail "High phase counter baseline is unavailable"
  high_control_offset=$(capture_append_offset "$control_ledger" 3145728)
  high_observation_offset=$(capture_append_offset "$observation_journal" 3145728)
  validate_host_memory_gate
  release_memory_loader high
  wait_for_asr_stop || fail "ASR did not stop through the real Governor/Supervisor path"
  wait_for_stable_asr_stop \
    || fail "ASR Stop was not stable beyond its restart delay"
  high_directive=$(wait_for_high_evidence \
    "$high_runtime_offset" "$high_control_offset" "$high_observation_offset" \
    "$high_baseline_high" "$high_app_generation" "$high_asr_generation") \
    || fail "High phase did not produce an exact Resource-to-ASR ledger chain"
  [ "$(unit_property "$SUPERVISOR_UNIT" MainPID)" = "$high_supervisor_pid" ] \
    || fail "Supervisor PID changed during the High phase"
  [ "$(unit_property "$SUPERVISOR_UNIT" InvocationID)" = "$high_supervisor_invocation" ] \
    || fail "Supervisor InvocationID changed during the High phase"

  # Explicitly stop and hash-clean the entire High phase before creating the independent Max phase.
  fixed_stop_app || fail "cannot stop the High app generation"
  app_started=0
  remove_e2e_assets || fail "cannot remove the High phase assets"
  /usr/bin/systemctl --user reset-failed "$APP_UNIT" >/dev/null 2>&1 || true
  /usr/bin/systemctl --user start "$ASR_UNIT" || fail "cannot restore ASR before the Max phase"
  wait_for_property "$ASR_UNIT" ActiveState active 30 || fail "ASR did not restart for Max"

  stage_memory_phase max
  max_runtime_offset=$(capture_append_offset "$runtime_ledger" 67108864)
  max_supervisor_pid=$(unit_property "$SUPERVISOR_UNIT" MainPID)
  max_supervisor_invocation=$(unit_property "$SUPERVISOR_UNIT" InvocationID)
  [ "$max_supervisor_pid:$max_supervisor_invocation" \
    = "$high_supervisor_pid:$high_supervisor_invocation" ] \
    || fail "Supervisor changed between the High and Max phases"
  app_started=1
  "$SUPERVISOR" launch || fail "hardened client could not launch the Max phase"
  wait_for_property "$APP_UNIT" ActiveState active 30 || fail "Max phase app did not start"
  max_app_generation=$(unit_property "$APP_UNIT" InvocationID)
  [ "$max_app_generation" != "$high_app_generation" ] \
    || fail "High and Max phases reused one app InvocationID"
  wait_for_loader_ready max || fail "Max loader did not reach its fixed ready gate"
  validate_phase_isolation max "$max_supervisor_pid" "$max_supervisor_invocation"
  max_loader_pid=$loader_pid
  max_main_pid=$app_main_pid
  max_webkit_pid=$webkit_pid
  max_loader_starttime=$(proc_starttime "$max_loader_pid") \
    || fail "cannot bind the Max loader PID to its /proc starttime"
  max_main_starttime=$(proc_starttime "$max_main_pid") \
    || fail "cannot bind the old app MainPID to its /proc starttime"
  max_webkit_starttime=$(proc_starttime "$max_webkit_pid") \
    || fail "cannot bind the old WebKit PID to its /proc starttime"
  wait_for_runtime_baseline "$max_runtime_offset" "$max_app_generation" \
    || fail "Max phase never established a trusted below-high Resource baseline"
  snapshot_line=$(snapshot_app_baseline) || fail "cannot capture the old app cgroup counters"
  IFS=' ' read -r \
    oom_generation snapshot_restarts high_before max_before oom_before oom_kill_before \
    oom_group_kill_before <<EOF
$snapshot_line
EOF
  [ "$oom_generation" = "$max_app_generation" ] \
    || fail "snapshot-app baseline generation changed before Max release"
  observation_offset=$(capture_append_offset "$observation_journal" 3145728)
  app_journal_cursor=$(capture_app_journal_cursor)
  restarts_before=$(unit_property "$APP_UNIT" NRestarts)
  case "$restarts_before" in ''|*[!0-9]*) fail "app NRestarts baseline is invalid" ;; esac
  [ "$snapshot_restarts" = "$restarts_before" ] \
    || fail "snapshot-app restart baseline changed before Max release"
  validate_host_memory_gate
  release_memory_loader max
  wait_for_app_generation_change "$oom_generation" \
    || fail "app cgroup did not cross the fixed MemoryMax boundary"
  wait_for_oom_evidence \
    "$observation_offset" "$app_journal_cursor" "$oom_generation" \
    "$high_before" "$max_before" "$oom_before" "$oom_kill_before" \
    "$oom_group_kill_before" "$restarts_before" \
    || fail "old-generation ExecStopPost OOM evidence did not reconcile across journald and Supervisor"
  wait_for_property "$APP_UNIT" ActiveState active 60 || fail "app did not perform its bounded restart"
  restarted_generation=$(unit_property "$APP_UNIT" InvocationID)
  [ "$restarted_generation" != "$oom_generation" ] || fail "OOM did not create a new app generation"
  expected_restarts=$((restarts_before + 1))
  [ "$(unit_property "$APP_UNIT" NRestarts)" = "$expected_restarts" ] \
    || fail "app restart count was not exactly one"
  pid_identity_retired "$max_loader_pid" "$max_loader_starttime" \
    || fail "memory.oom.group did not retire the old loader process identity"
  pid_identity_retired "$max_main_pid" "$max_main_starttime" \
    || fail "memory.oom.group did not retire the old app MainPID identity"
  pid_identity_retired "$max_webkit_pid" "$max_webkit_starttime" \
    || fail "memory.oom.group did not retire the old WebKit process identity"
  assert_receipt pinvou_app status "$SUPERVISOR" status
  [ "$(unit_property "$SUPERVISOR_UNIT" MainPID)" = "$max_supervisor_pid" ] \
    || fail "Supervisor restarted during the app cgroup OOM"
  [ "$(unit_property "$SUPERVISOR_UNIT" InvocationID)" = "$max_supervisor_invocation" ] \
    || fail "Supervisor InvocationID changed during the app cgroup OOM"
  validate_fixed_file "$max_once_marker" "$uid" 600 "$MAX_ONCE_SHA256"
  validate_effective_app_profile
  restarted_cgroup=$(unit_property "$APP_UNIT" ControlGroup)
  supervisor_cgroup_after=$(unit_property "$SUPERVISOR_UNIT" ControlGroup)
  case "$supervisor_cgroup_after" in
    "$restarted_cgroup"|"$restarted_cgroup"/*)
      fail "Supervisor entered the restarted app cgroup subtree"
      ;;
  esac
  [ "$(proc_unified_cgroup "$max_supervisor_pid")" = "$supervisor_cgroup_after" ] \
    || fail "Supervisor cgroup identity changed after app OOM"
  wait_for_webkit_in_app_cgroup "$restarted_cgroup" >/dev/null \
    || fail "restarted app did not restore a real WebKit child in its cgroup"
  wait_for_stable_restarted_app "$restarted_generation" "$expected_restarts" \
    || fail "restarted app generation was not stable for 30 seconds"
  /usr/bin/printf '%s\n' \
    "memory-max-e2e-pass highDirective=$high_directive oldAppInvocationID=$oom_generation newAppInvocationID=$restarted_generation supervisorMainPID=$max_supervisor_pid"
}

prepare_purge() {
  [ "$#" -eq 0 ] || fail "prepare-purge takes no arguments"
  ensure_state_directory
  recover_all_staging_orphans || fail "cannot safely recover a prior E2E staging orphan"
  load_and_verify_installed_package
  [ -x "$PROFILE_HELPER" ] || fail "run prepare-purge before removing the package"
  fixed_stop_app || fail "cannot stop the fixed app before profile deactivation"
  remove_e2e_assets || fail "cannot hash-clean fixed E2E assets before purge"
  /usr/bin/systemctl --user daemon-reload || fail "cannot reload after E2E asset removal"
  /usr/bin/systemctl --user reset-failed "$APP_UNIT" >/dev/null 2>&1 || true
  "$PROFILE_HELPER" deactivate >/dev/null
  [ "$("$PROFILE_HELPER" status)" = inactive ] || fail "profile is still active"
  for target in \
    "$profile_target" "$desktop_target" \
    "$profile_staging_dir" "$desktop_staging_dir" "$marker_staging_dir" \
    "$legacy_profile_marker" "$installing_profile_marker" "$applied_profile_marker" \
    "$profile_quarantine" "$desktop_quarantine" \
    "$legacy_marker_quarantine" "$installing_marker_quarantine" \
    "$applied_marker_quarantine"; do
    [ ! -e "$target" ] && [ ! -L "$target" ] \
      || fail "helper-owned file remains before purge: $target"
  done
  assert_no_e2e_assets
  assert_no_transaction_residue
  [ "$(unit_property "$ASR_UNIT" ActiveState)" = active ] \
    || fail "ASR must be active at the purge boundary"
  purge_asr_invocation=$(unit_property "$ASR_UNIT" InvocationID)
  purge_asr_pid=$(unit_property "$ASR_UNIT" MainPID)
  case "$purge_asr_invocation" in ''|*[!0-9a-f]*) fail "purge ASR InvocationID is invalid" ;; esac
  [ "${#purge_asr_invocation}" -eq 32 ] || fail "purge ASR InvocationID is invalid"
  case "$purge_asr_pid" in ''|0|*[!0-9]*) fail "purge ASR MainPID is invalid" ;; esac
  purge_tmp=$(/usr/bin/mktemp "$e2e_state_dir/.purge.XXXXXX") \
    || fail "cannot stage the purge evidence marker"
  /usr/bin/printf '%s\n' \
    'schema=pinvou-megabook-purge-v1' \
    "deb_sha256=$(baseline_value deb_sha256)" \
    "deb_version=$(baseline_value deb_version)" \
    "asr_invocation_id=$purge_asr_invocation" \
    "asr_main_pid=$purge_asr_pid" >"$purge_tmp"
  validate_user_file "$purge_tmp" 600
  if [ ! -e "$purge_file" ] && [ ! -L "$purge_file" ]; then
    publish_private_staged_file "$purge_tmp" "$purge_file" "$e2e_state_dir"
    purge_tmp=
  elif [ ! -L "$purge_file" ] && [ -f "$purge_file" ] \
      && validate_user_file "$purge_file" 600 \
      && /usr/bin/cmp -s -- "$purge_tmp" "$purge_file"; then
    fsync_file "$purge_tmp"
    /usr/bin/rm -- "$purge_tmp" || fail "cannot retire duplicate purge staging file"
    purge_tmp=
    fsync_directory "$e2e_state_dir"
  else
    /usr/bin/rm -f -- "$purge_tmp" || true
    purge_tmp=
    fsync_directory "$e2e_state_dir"
    fail "a different purge boundary marker already exists"
  fi
  /usr/bin/printf '%s\n' \
    'purge-ready: run sudo apt-get purge pinvou3, then verify-purged before reinstalling'
}

purge_value() {
  key=$1
  value=$(/usr/bin/awk -F= -v key="$key" '$1 == key { sub(/^[^=]*=/, ""); print; count += 1 } END { if (count != 1) exit 1 }' "$purge_file") \
    || fail "purge evidence key is missing or duplicated: $key"
  [ -n "$value" ] || fail "purge evidence key is empty: $key"
  /usr/bin/printf '%s\n' "$value"
}

verify_purged() {
  [ "$#" -eq 0 ] || fail "verify-purged takes no arguments"
  recover_all_staging_orphans || fail "cannot safely recover a prior E2E staging orphan"
  validate_user_file "$baseline_file" 600
  validate_user_file "$purge_file" 600
  [ "$(purge_value schema)" = pinvou-megabook-purge-v1 ] || fail "purge evidence schema mismatch"
  [ "$(purge_value deb_sha256)" = "$(baseline_value deb_sha256)" ] \
    || fail "purge evidence does not match the install baseline"
  [ "$(purge_value deb_version)" = "$(baseline_value deb_version)" ] \
    || fail "purge version does not match the install baseline"
  [ -z "$(/usr/bin/dpkg-query -W -f='${Status}' pinvou3 2>/dev/null || true)" ] \
    || fail "dpkg still retains pinvou3 after purge"

  for target in \
    /usr/bin/pinvou3-tauri \
    "$SUPERVISOR" "$PROFILE_HELPER" "$GENERIC_DESKTOP" \
    /usr/lib/systemd/user/pinvou3-supervisor.socket \
    /usr/lib/systemd/user/pinvou3-supervisor.service \
    /usr/lib/systemd/user/pinvou3-app.service \
    /usr/lib/systemd/user/pinvou-qwen3-asr.service.d/50-pinvou-supervisor.conf \
    /usr/share/pinvou3/supervisor/descriptors/pinvou-app-v1.json \
    /usr/share/pinvou3/supervisor/descriptors/pinvou-asr-v1.json \
    /usr/share/pinvou3/supervisor/profiles/megabook-canary.conf \
    /usr/share/pinvou3/supervisor/profiles/pinvou3-megabook-canary.desktop \
    /usr/share/applications/pinvou3-megabook-canary.desktop \
    "$runtime_dir/pinvou-supervisor/control.sock" \
    "$profile_target" "$desktop_target" \
    "$profile_staging_dir" "$desktop_staging_dir" "$marker_staging_dir" \
    "$legacy_profile_marker" "$installing_profile_marker" "$applied_profile_marker" \
    "$profile_quarantine" "$desktop_quarantine" \
    "$legacy_marker_quarantine" "$installing_marker_quarantine" \
    "$applied_marker_quarantine"; do
    [ ! -e "$target" ] && [ ! -L "$target" ] || fail "purged path remains: $target"
  done
  assert_no_e2e_assets
  assert_no_transaction_residue
  for removed_unit in "$APP_UNIT" "$SUPERVISOR_UNIT" "$SOCKET_UNIT"; do
    [ "$(unit_property "$removed_unit" LoadState 2>/dev/null)" = not-found ] \
      || fail "removed user unit remains loaded: $removed_unit"
  done

  asr_unit_path=$home_dir/.config/systemd/user/pinvou-qwen3-asr.service
  asr_data_dir=$home_dir/.pinvou3/asr/qwen3-asr-openvino
  validate_user_file "$asr_unit_path" 600
  validate_owned_directory "$asr_data_dir"
  [ -x "$asr_data_dir/runtime/bin/python" ] || fail "ASR runtime disappeared during purge"
  [ -f "$asr_data_dir/qwen3-asr-openvino.py" ] || fail "ASR service data disappeared during purge"
  [ "$(unit_property "$ASR_UNIT" FragmentPath)" = "$asr_unit_path" ] \
    || fail "ASR base unit identity changed during purge"
  [ "$(unit_property "$ASR_UNIT" ActiveState)" = active ] || fail "ASR is not active after purge"
  [ "$(unit_property "$ASR_UNIT" InvocationID)" = "$(purge_value asr_invocation_id)" ] \
    || fail "purge restarted the ASR instance"
  [ "$(unit_property "$ASR_UNIT" MainPID)" = "$(purge_value asr_main_pid)" ] \
    || fail "purge changed the ASR MainPID"

  /usr/bin/rm -- "$purge_file" "$baseline_file"
  /usr/bin/rmdir -- "$e2e_state_dir" \
    || fail "dedicated E2E state directory contains an unexpected residue"
  /usr/bin/printf '%s\n' 'purge-e2e-pass: package assets are absent and ASR remained intact'
}

[ "$#" -ge 1 ] || fail "usage: megabook-supervisor-e2e <baseline|verify-safe|verify-memory-max|prepare-purge|verify-purged>"
operation=$1
shift
case "$operation" in
  baseline) baseline "$@" ;;
  verify-safe)
    [ "$#" -eq 0 ] || fail "verify-safe takes no arguments"
    verify_common
    ;;
  verify-memory-max)
    [ "$#" -eq 0 ] || fail "verify-memory-max takes no arguments"
    verify_memory_max
    ;;
  prepare-purge) prepare_purge "$@" ;;
  verify-purged) verify_purged "$@" ;;
  *) fail "usage: megabook-supervisor-e2e <baseline|verify-safe|verify-memory-max|prepare-purge|verify-purged>" ;;
esac
