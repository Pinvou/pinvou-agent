#!/bin/sh
# Explicit, user-owned activation for the fixed MegaBook canary profile.
#
# The helper accepts no path, unit, property, PID, or command input.
#
# VERSIONED CLEANUP ABI: the v1 profile/desktop package sources, per-user public targets, byte
# lengths, SHA-256 digests, and legacy marker path/bytes below are immutable. A content or path
# change MUST use new v2 source/target paths and a new v2 ownership marker. The v1 constants and
# cleanup allowlist must remain here so installations created by the old helper stay recoverable.
# The legacy marker bytes are exactly the three LEGACY_MARKER_*_LINE values, each followed by LF.

set -eu
umask 077
LC_ALL=C
export LC_ALL

PROFILE_SOURCE=/usr/share/pinvou3/supervisor/profiles/megabook-canary.conf
DESKTOP_SOURCE=/usr/share/pinvou3/supervisor/profiles/pinvou3-megabook-canary.desktop
PROFILE_TARGET_SUFFIX=/.config/systemd/user/pinvou3-app.service.d/50-megabook-canary.conf
DESKTOP_TARGET_SUFFIX=/.local/share/applications/pinvou3-megabook-canary.desktop
LEGACY_MARKER_TARGET_SUFFIX=/.local/state/pinvou3/megabook-profile-v1.registered
PROFILE_BYTES=351
DESKTOP_BYTES=465
PROFILE_SHA256=74cc705379e10f6626bb614118e66c080366e3bed907509a786d7692048e451c
DESKTOP_SHA256=ddfe6a25920570d8992a9eb6c3d53bcc64404a6ce069e764e42383141e9a12a0
LEGACY_MARKER_SCHEMA_LINE=schema=pinvou-megabook-profile-v1
LEGACY_MARKER_PROFILE_LINE=profile_sha256=74cc705379e10f6626bb614118e66c080366e3bed907509a786d7692048e451c
LEGACY_MARKER_DESKTOP_LINE=desktop_sha256=ddfe6a25920570d8992a9eb6c3d53bcc64404a6ce069e764e42383141e9a12a0
LEGACY_MARKER_BYTES=194
INSTALLING_MARKER_BYTES=211
APPLIED_MARKER_BYTES=208
LEGACY_MARKER_SHA256=5858fdf923bace7a8895b7a901f5ac16d798a97e7c15d8a361533329f9c605cc
INSTALLING_MARKER_SHA256=02e599747fdf54301cf8f77227e4668f4e5a5112817b29a9cfe786c4613d98b5
APPLIED_MARKER_SHA256=efd76b9543fcec1b362047e7b8b0fee91773811e35b2a3352224ae44dca7f6d3
APP_UNIT=pinvou3-app.service
APP_FRAGMENT=/usr/lib/systemd/user/pinvou3-app.service
SUPERVISOR=/usr/lib/pinvou3/supervisor/pinvou-supervisor

profile_dir=
desktop_dir=
state_dir=
profile_staging_dir=
desktop_staging_dir=
marker_staging_dir=
registration_kind=
cleanup_trap_ready=0

fail() {
  /usr/bin/printf '%s\n' "pinvou-megabook-profile: $*" >&2
  exit 1
}

file_identity() {
  /usr/bin/stat -c %d:%i -- "$1"
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
  fsync_path file "$1" || fail "cannot fsync staged file: $1"
}

fsync_directory() {
  fsync_path directory "$1" || fail "cannot fsync directory: $1"
}

for required_tool in /usr/bin/getent /usr/bin/id /usr/bin/install /usr/bin/ln \
  /usr/bin/mkdir /usr/bin/mktemp /usr/bin/mv /usr/bin/printf /usr/bin/python3 \
  /usr/bin/rm /usr/bin/sha256sum /usr/bin/stat /usr/bin/systemctl; do
  [ -x "$required_tool" ] || fail "required fixed tool is unavailable: $required_tool"
done

uid=$(/usr/bin/id -u) || fail "cannot determine the effective uid"
case "$uid" in
  ''|*[!0-9]*|0) fail "must run as a non-root login user" ;;
esac

passwd_record=$(/usr/bin/getent passwd "$uid") || fail "cannot resolve the effective uid"
case "$passwd_record" in
  *"
"*) fail "passwd lookup returned more than one record" ;;
esac
login_name=${passwd_record%%:*}
passwd_tail=${passwd_record#*:}
passwd_tail=${passwd_tail#*:}
passwd_uid=${passwd_tail%%:*}
passwd_tail=${passwd_tail#*:}
passwd_gid=${passwd_tail%%:*}
passwd_tail=${passwd_tail#*:}
passwd_tail=${passwd_tail#*:}
home_dir=${passwd_tail%%:*}
[ "$passwd_uid" = "$uid" ] || fail "passwd uid does not match the effective uid"
primary_gid=$(/usr/bin/id -g) || fail "cannot determine the effective primary gid"
case "$passwd_gid" in ''|*[!0-9]*) fail "passwd gid is invalid" ;; esac
case "$primary_gid" in ''|*[!0-9]*) fail "effective primary gid is invalid" ;; esac
[ "$passwd_gid" = "$primary_gid" ] || fail "passwd gid does not match the effective primary gid"

# Some desktop environments create ~/.config/systemd{,/user} as 0775. Group-write is trusted only
# when the directory uses a proven user-private primary group: no supplementary group member and
# no other passwd identity has that primary gid. Other-write remains forbidden everywhere.
primary_group_record=$(/usr/bin/getent group "$primary_gid") \
  || fail "cannot resolve the effective primary group"
case "$primary_group_record" in
  *"
"*) fail "primary group lookup returned more than one record" ;;
esac
primary_group_tail=${primary_group_record#*:}
primary_group_tail=${primary_group_tail#*:}
resolved_primary_gid=${primary_group_tail%%:*}
primary_group_members=${primary_group_tail#*:}
[ "$resolved_primary_gid" = "$primary_gid" ] \
  || fail "primary group lookup returned a different gid"
case "$primary_group_members" in
  ''|"$login_name") ;;
  *) fail "effective primary group is shared by another supplementary member" ;;
esac
passwd_database=$(/usr/bin/getent passwd) || fail "cannot enumerate primary-gid ownership"
/usr/bin/printf '%s\n' "$passwd_database" | /usr/bin/python3 -I -c '
import sys

uid, gid, login = sys.argv[1:]
raw = sys.stdin.buffer.read(1024 * 1024 + 1)
if not raw or len(raw) > 1024 * 1024:
    raise SystemExit("passwd database is empty or exceeds the fixed validation bound")
try:
    text = raw.decode("utf-8")
except UnicodeDecodeError as error:
    raise SystemExit("passwd database is not UTF-8: " + str(error))
owners = []
for line in text.splitlines():
    fields = line.split(":")
    if len(fields) != 7:
        raise SystemExit("passwd database contains a malformed record")
    if fields[3] == gid:
        owners.append((fields[0], fields[2], fields[3]))
if owners != [(login, uid, gid)]:
    raise SystemExit("effective primary gid is shared by another passwd identity")
' "$uid" "$primary_gid" "$login_name" \
  || fail "effective primary group is not provably user-private"
passwd_database=
case "$home_dir" in
  /|''|*'//'*) fail "login home is not a bounded absolute path" ;;
  /*) ;;
  *) fail "login home is not absolute" ;;
esac
case "/${home_dir#/}/" in
  *'/../'*|*'/./'*) fail "login home contains an unsafe path component" ;;
esac

profile_target=$home_dir$PROFILE_TARGET_SUFFIX
profile_dir=${profile_target%/*}
profile_quarantine=$profile_dir/.pinvou-quarantine-profile-v2
desktop_target=$home_dir$DESKTOP_TARGET_SUFFIX
desktop_dir=${desktop_target%/*}
desktop_quarantine=$desktop_dir/.pinvou-quarantine-desktop-v2
legacy_marker_target=$home_dir$LEGACY_MARKER_TARGET_SUFFIX
state_dir=${legacy_marker_target%/*}
installing_marker_target=$state_dir/megabook-profile-v2.installing
applied_marker_target=$state_dir/megabook-profile-v2.applied
legacy_marker_quarantine=$state_dir/.pinvou-quarantine-marker-v1
installing_marker_quarantine=$state_dir/.pinvou-quarantine-marker-v2-installing
applied_marker_quarantine=$state_dir/.pinvou-quarantine-marker-v2-applied
profile_staging_dir=$profile_dir/.pinvou-profile-staging-v2
desktop_staging_dir=$desktop_dir/.pinvou-desktop-staging-v2
marker_staging_dir=$state_dir/.pinvou-marker-staging-v2

validate_owned_directory() {
  owned_directory=$1
  [ ! -L "$owned_directory" ] \
    || fail "directory must not be a symlink: $owned_directory"
  [ -d "$owned_directory" ] || fail "directory is not present: $owned_directory"
  owned_directory_before=$(/usr/bin/stat -c %d:%i:%u:%g:%a -- "$owned_directory") \
    || fail "cannot inspect directory metadata: $owned_directory"
  owned_directory_mode=${owned_directory_before##*:}
  owned_directory_metadata=${owned_directory_before%:*}
  owned_directory_gid=${owned_directory_metadata##*:}
  owned_directory_metadata=${owned_directory_metadata%:*}
  owned_directory_uid=${owned_directory_metadata##*:}
  [ "$owned_directory_uid" = "$uid" ] \
    || fail "directory owner does not match the effective uid: $owned_directory"
  case "$owned_directory_gid" in
    ''|*[!0-9]*) fail "directory gid is invalid: $owned_directory" ;;
  esac
  case "$owned_directory_mode" in
    ''|*[!0-7]*) fail "directory mode is invalid: $owned_directory" ;;
  esac
  owned_directory_mode_value=$((0$owned_directory_mode))
  [ $((owned_directory_mode_value & 0002)) -eq 0 ] \
    || fail "directory is other-writable: $owned_directory"
  if [ $((owned_directory_mode_value & 0020)) -ne 0 ]; then
    [ "$owned_directory_gid" = "$primary_gid" ] \
      || fail "group-writable directory does not use the private primary group: $owned_directory"
  fi
  [ ! -L "$owned_directory" ] \
    && [ "$owned_directory_before" \
      = "$(/usr/bin/stat -c %d:%i:%u:%g:%a -- "$owned_directory")" ] \
    || fail "directory changed during metadata validation: $owned_directory"
}

validate_optional_owned_directory() {
  optional_directory=$1
  if [ ! -e "$optional_directory" ] && [ ! -L "$optional_directory" ]; then
    return 1
  fi
  validate_owned_directory "$optional_directory"
}

# Public targets are reachable only through these three fixed per-user chains. Validate every
# existing component, not merely the final parent: otherwise replacing `.config` or `.local` with
# a symlink could redirect status/deactivation and quarantine deletion outside the registered ABI.
validate_existing_fixed_directory_chains() {
  validate_owned_directory "$home_dir"
  if validate_optional_owned_directory "$home_dir/.config"; then
    if validate_optional_owned_directory "$home_dir/.config/systemd"; then
      if validate_optional_owned_directory "$home_dir/.config/systemd/user"; then
        validate_optional_owned_directory "$profile_dir" || true
      fi
    fi
  fi
  if validate_optional_owned_directory "$home_dir/.local"; then
    if validate_optional_owned_directory "$home_dir/.local/share"; then
      validate_optional_owned_directory "$desktop_dir" || true
    fi
    if validate_optional_owned_directory "$home_dir/.local/state"; then
      validate_optional_owned_directory "$state_dir" || true
    fi
  fi
  return 0
}

validate_fixed_parent_chain() {
  chain_directory=$1
  validate_owned_directory "$home_dir"
  case "$chain_directory" in
    "$profile_dir")
      for chain_component in \
        "$home_dir/.config" \
        "$home_dir/.config/systemd" \
        "$home_dir/.config/systemd/user" \
        "$profile_dir"; do
        validate_owned_directory "$chain_component"
      done
      ;;
    "$desktop_dir")
      for chain_component in \
        "$home_dir/.local" \
        "$home_dir/.local/share" \
        "$desktop_dir"; do
        validate_owned_directory "$chain_component"
      done
      ;;
    "$state_dir")
      for chain_component in \
        "$home_dir/.local" \
        "$home_dir/.local/state" \
        "$state_dir"; do
        validate_owned_directory "$chain_component"
      done
      ;;
    *) fail "internal fixed parent chain is not registered: $chain_directory" ;;
  esac
}

fixed_parent_chain_identity() {
  identity_directory=$1
  validate_fixed_parent_chain "$identity_directory"
  identity_result=$(file_identity "$home_dir") \
    || fail "cannot identify the fixed home directory"
  case "$identity_directory" in
    "$profile_dir")
      for identity_component in \
        "$home_dir/.config" \
        "$home_dir/.config/systemd" \
        "$home_dir/.config/systemd/user" \
        "$profile_dir"; do
        identity_result=$identity_result:$(file_identity "$identity_component") \
          || fail "cannot identify fixed profile directory chain: $identity_component"
      done
      ;;
    "$desktop_dir")
      for identity_component in \
        "$home_dir/.local" \
        "$home_dir/.local/share" \
        "$desktop_dir"; do
        identity_result=$identity_result:$(file_identity "$identity_component") \
          || fail "cannot identify fixed desktop directory chain: $identity_component"
      done
      ;;
    "$state_dir")
      for identity_component in \
        "$home_dir/.local" \
        "$home_dir/.local/state" \
        "$state_dir"; do
        identity_result=$identity_result:$(file_identity "$identity_component") \
          || fail "cannot identify fixed marker directory chain: $identity_component"
      done
      ;;
  esac
  /usr/bin/printf '%s\n' "$identity_result"
}

fixed_public_parent() {
  case "$1" in
    "$profile_target"|"$profile_quarantine") /usr/bin/printf '%s\n' "$profile_dir" ;;
    "$desktop_target"|"$desktop_quarantine") /usr/bin/printf '%s\n' "$desktop_dir" ;;
    "$legacy_marker_target"|"$installing_marker_target"|"$applied_marker_target"|\
    "$legacy_marker_quarantine"|"$installing_marker_quarantine"|\
    "$applied_marker_quarantine") /usr/bin/printf '%s\n' "$state_dir" ;;
    *) fail "internal public file is not in the fixed profile ABI: $1" ;;
  esac
}

validate_fixed_public_file_one_of() {
  public_file=$1
  public_mode=$2
  public_allowed_links=$3
  shift 3
  public_parent=$(fixed_public_parent "$public_file") \
    || fail "cannot resolve fixed public-file parent: $public_file"
  public_chain_identity=$(fixed_parent_chain_identity "$public_parent") \
    || fail "cannot validate fixed public-file directory chain: $public_file"
  validate_owned_file_one_of \
    "$public_file" "$public_mode" "$public_allowed_links" "$@"
  [ "$public_chain_identity" = "$(fixed_parent_chain_identity "$public_parent")" ] \
    || fail "fixed directory chain changed while validating public file: $public_file"
}

validate_fixed_public_file() {
  validate_fixed_public_file_one_of "$1" "$2" 1 "$3"
}

ensure_owned_directory() {
  directory=$1
  if [ ! -e "$directory" ]; then
    /usr/bin/mkdir -m 0700 -- "$directory" || fail "cannot create directory: $directory"
    fsync_directory "${directory%/*}"
  fi
  validate_owned_directory "$directory"
}

validate_source() {
  source_file=$1
  expected_sha=$2
  expected_bytes=$3
  [ ! -L "$source_file" ] && [ -f "$source_file" ] \
    || fail "package source is missing or not a regular file: $source_file"
  before=$(/usr/bin/stat -c %d:%i:%u:%g:%a:%h -- "$source_file") \
    || fail "cannot lstat package source: $source_file"
  case "$before" in
    *:0:0:644:1) ;;
    *) fail "package source owner/mode/link count is not trusted: $source_file" ;;
  esac
  [ "$(/usr/bin/stat -c %s -- "$source_file")" = "$expected_bytes" ] \
    || fail "package source byte length mismatch: $source_file"
  source_digest=$(/usr/bin/sha256sum "$source_file") || fail "cannot hash package source"
  source_digest=${source_digest%% *}
  [ "$source_digest" = "$expected_sha" ] || fail "package source hash mismatch: $source_file"
  [ ! -L "$source_file" ] && [ "$before" = "$(/usr/bin/stat -c %d:%i:%u:%g:%a:%h -- "$source_file")" ] \
    || fail "package source changed during validation: $source_file"
}

validate_owned_file_one_of() {
  owned_target_file=$1
  owned_expected_mode=$2
  owned_allowed_links=$3
  shift 3
  [ ! -L "$owned_target_file" ] && [ -f "$owned_target_file" ] \
    || fail "owned path is missing or not a regular file: $owned_target_file"
  owned_before=$(/usr/bin/stat -c %d:%i:%u:%a:%h -- "$owned_target_file") \
    || fail "cannot lstat owned file: $owned_target_file"
  owned_metadata_tail=${owned_before#*:*:}
  owned_owner=${owned_metadata_tail%%:*}
  owned_metadata_tail=${owned_metadata_tail#*:}
  owned_mode=${owned_metadata_tail%%:*}
  owned_links=${owned_metadata_tail##*:}
  [ "$owned_owner:$owned_mode" = "$uid:$owned_expected_mode" ] \
    || fail "owned file owner/mode changed: $owned_target_file"
  case ":$owned_allowed_links:" in
    *":$owned_links:"*) ;;
    *) fail "owned file link count changed: $owned_target_file" ;;
  esac
  owned_digest=$(/usr/bin/sha256sum "$owned_target_file") \
    || fail "cannot hash owned file: $owned_target_file"
  owned_digest=${owned_digest%% *}
  owned_matched=0
  for owned_expected_sha in "$@"; do
    [ "$owned_digest" = "$owned_expected_sha" ] && owned_matched=1
  done
  [ "$owned_matched" -eq 1 ] || fail "owned file content changed: $owned_target_file"
  [ ! -L "$owned_target_file" ] \
    && [ "$owned_before" = "$(/usr/bin/stat -c %d:%i:%u:%a:%h -- "$owned_target_file")" ] \
    || fail "owned file changed during validation: $owned_target_file"
}

validate_owned_file() {
  validate_owned_file_one_of "$1" "$2" 1 "$3"
}

RESERVED_STAGING_ALLOWED_LINKS=1:2

validate_private_staging_directory() {
  staging_directory=$1
  parent_directory=$2
  [ "${staging_directory%/*}" = "$parent_directory" ] \
    || fail "reserved staging namespace escaped its fixed parent: $staging_directory"
  [ ! -L "$staging_directory" ] && [ -d "$staging_directory" ] \
    || fail "reserved staging namespace is not a real directory: $staging_directory"
  [ "$(/usr/bin/stat -c %u:%a -- "$staging_directory")" = "$uid:700" ] \
    || fail "reserved staging namespace owner/mode is not private: $staging_directory"
}

prepare_private_staging_directory() {
  staging_directory=$1
  parent_directory=$2
  prepare_chain_identity=$(fixed_parent_chain_identity "$parent_directory") \
    || fail "cannot validate fixed staging parent chain: $parent_directory"
  [ ! -e "$staging_directory" ] && [ ! -L "$staging_directory" ] \
    || fail "reserved staging namespace already exists; preserved recovery path: $staging_directory"
  /usr/bin/mkdir -m 0700 -- "$staging_directory" \
    || fail "cannot create reserved staging namespace: $staging_directory"
  fsync_directory "$parent_directory"
  [ "$prepare_chain_identity" = "$(fixed_parent_chain_identity "$parent_directory")" ] \
    || fail "fixed staging parent chain changed during namespace creation: $parent_directory"
  validate_private_staging_directory "$staging_directory" "$parent_directory"
}

# A SIGKILL may leave mktemp's empty 0600 inode or install's partially-written 0644 inode.
# Those nlink=1 files are safe to unlink only because this fixed 0700 namespace is reserved to
# this helper. An nlink=2 file is a publication record: recovery requires its complete pinned
# bytes and the same inode at its one fixed public target. Unknown entries are never recursed
# into or removed; the diagnostic names the exact path for manual recovery.
cleanup_reserved_staging_set() {
  staging_directory=$1
  parent_directory=$2
  staging_prefix=$3
  allowed_modes=$4
  allowed_links=$5
  shift 5
  /usr/bin/python3 -I - "$uid" "$primary_gid" "$staging_directory" "$parent_directory" \
    "$staging_prefix" "$allowed_modes" "$allowed_links" "$@" <<'PY'
import hashlib
import os
import re
import stat
import sys


class RecoveryError(Exception):
    pass


def preserve(path, reason):
    raise RecoveryError(f"{reason}; preserved recovery path: {path}")


def identity(metadata):
    return (
        metadata.st_dev,
        metadata.st_ino,
        metadata.st_uid,
        metadata.st_gid,
        stat.S_IMODE(metadata.st_mode),
        metadata.st_nlink,
        metadata.st_size,
    )


def digest_fd(fd):
    os.lseek(fd, 0, os.SEEK_SET)
    digest = hashlib.sha256()
    while True:
        chunk = os.read(fd, 65536)
        if not chunk:
            return digest.hexdigest()
        digest.update(chunk)


def open_stable_regular(directory_fd, name, expected, display_path):
    flags = (
        os.O_RDONLY
        | os.O_NONBLOCK
        | getattr(os, "O_CLOEXEC", 0)
        | getattr(os, "O_NOFOLLOW", 0)
    )
    try:
        fd = os.open(name, flags, dir_fd=directory_fd)
    except OSError as error:
        preserve(display_path, f"cannot open staging recovery file: {error}")
    metadata = os.fstat(fd)
    try:
        named = os.stat(name, dir_fd=directory_fd, follow_symlinks=False)
    except OSError as error:
        os.close(fd)
        preserve(display_path, f"staging recovery name changed: {error}")
    if not stat.S_ISREG(metadata.st_mode) or identity(metadata) != expected or identity(named) != expected:
        os.close(fd)
        preserve(display_path, "staging recovery inode changed")
    return fd


def main():
    uid_text, private_gid_text, staging_path, parent_path, prefix, allowed_text, \
        links_text, *raw_mappings = sys.argv[1:]
    uid = int(uid_text)
    private_gid = int(private_gid_text)
    if os.path.dirname(staging_path) != parent_path or not os.path.isabs(staging_path):
        preserve(staging_path, "reserved staging namespace escaped its fixed parent")
    if len(raw_mappings) == 0 or len(raw_mappings) % 4:
        preserve(staging_path, "internal staging recovery map is invalid")
    try:
        allowed = {int(value, 8) for value in allowed_text.split(":")}
        allowed_link_counts = {int(value) for value in links_text.split(":")}
        mappings = [
            {
                "mode": int(raw_mappings[index], 8),
                "size": int(raw_mappings[index + 1]),
                "sha256": raw_mappings[index + 2],
                "target": raw_mappings[index + 3],
            }
            for index in range(0, len(raw_mappings), 4)
        ]
    except ValueError:
        preserve(staging_path, "internal staging recovery map is invalid")
    if allowed_link_counts != {1, 2}:
        preserve(staging_path, "internal staging recovery link allowlist is invalid")

    try:
        initial_stage = os.lstat(staging_path)
    except FileNotFoundError:
        return
    except OSError as error:
        preserve(staging_path, f"cannot inspect reserved staging namespace: {error}")
    try:
        initial_parent = os.lstat(parent_path)
    except OSError as error:
        preserve(staging_path, f"cannot inspect fixed staging parent: {error}")
    parent_mode = stat.S_IMODE(initial_parent.st_mode)
    if not stat.S_ISDIR(initial_parent.st_mode) or initial_parent.st_uid != uid \
            or parent_mode & 0o002 \
            or (parent_mode & 0o020 and initial_parent.st_gid != private_gid):
        preserve(staging_path, "fixed staging parent metadata is not trusted")
    if not stat.S_ISDIR(initial_stage.st_mode) or initial_stage.st_uid != uid \
            or stat.S_IMODE(initial_stage.st_mode) != 0o700:
        preserve(staging_path, "reserved staging namespace metadata is not trusted")

    directory_flags = (
        os.O_RDONLY
        | getattr(os, "O_CLOEXEC", 0)
        | getattr(os, "O_NOFOLLOW", 0)
        | getattr(os, "O_DIRECTORY", 0)
    )
    parent_fd = os.open(parent_path, directory_flags)
    staging_fd = None
    try:
        if identity(os.fstat(parent_fd)) != identity(initial_parent):
            preserve(staging_path, "fixed staging parent changed while opening")
        staging_name = os.path.basename(staging_path)
        named_stage = os.stat(staging_name, dir_fd=parent_fd, follow_symlinks=False)
        if identity(named_stage) != identity(initial_stage):
            preserve(staging_path, "reserved staging namespace changed during validation")
        staging_fd = os.open(staging_name, directory_flags, dir_fd=parent_fd)
        if identity(os.fstat(staging_fd)) != identity(initial_stage):
            preserve(staging_path, "reserved staging namespace changed while opening")

        names = sorted(os.listdir(staging_fd))
        pattern = re.compile(re.escape(prefix) + r"\.[A-Za-z0-9]{6}\Z")
        plans = []
        for name in names:
            candidate_path = os.path.join(staging_path, name)
            if pattern.fullmatch(name) is None:
                preserve(candidate_path, "unrecognized reserved staging name")
            try:
                metadata = os.stat(name, dir_fd=staging_fd, follow_symlinks=False)
            except OSError as error:
                preserve(candidate_path, f"cannot inspect staging recovery file: {error}")
            mode = stat.S_IMODE(metadata.st_mode)
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != uid:
                preserve(candidate_path, "staging recovery file type/owner is not trusted")
            if mode not in allowed or metadata.st_nlink not in allowed_link_counts:
                preserve(candidate_path, "staging recovery file mode/link count is not trusted")
            expected_identity = identity(metadata)
            if metadata.st_nlink == 1:
                fd = open_stable_regular(staging_fd, name, expected_identity, candidate_path)
                os.close(fd)
                plans.append((name, expected_identity, None))
                continue

            fd = open_stable_regular(staging_fd, name, expected_identity, candidate_path)
            try:
                candidate_digest = digest_fd(fd)
            finally:
                os.close(fd)
            matching = [
                mapping
                for mapping in mappings
                if mapping["mode"] == mode
                and mapping["size"] == metadata.st_size
                and mapping["sha256"] == candidate_digest
            ]
            if len(matching) != 1:
                preserve(candidate_path, "published staging link does not have pinned complete bytes")
            mapping = matching[0]
            target_path = mapping["target"]
            if os.path.dirname(target_path) != parent_path:
                preserve(candidate_path, "published staging link has no fixed public target")
            target_name = os.path.basename(target_path)
            try:
                target_metadata = os.stat(target_name, dir_fd=parent_fd, follow_symlinks=False)
            except OSError as error:
                preserve(candidate_path, f"fixed public target is unavailable: {target_path}: {error}")
            target_expected = identity(target_metadata)
            if not stat.S_ISREG(target_metadata.st_mode) \
                    or target_metadata.st_uid != uid \
                    or stat.S_IMODE(target_metadata.st_mode) != mapping["mode"] \
                    or target_metadata.st_nlink != 2 \
                    or target_metadata.st_size != mapping["size"] \
                    or target_metadata.st_dev != metadata.st_dev \
                    or target_metadata.st_ino != metadata.st_ino:
                preserve(candidate_path, f"published staging link is not the fixed public target inode: {target_path}")
            target_fd = open_stable_regular(parent_fd, target_name, target_expected, target_path)
            try:
                target_digest = digest_fd(target_fd)
            finally:
                os.close(target_fd)
            if target_digest != mapping["sha256"]:
                preserve(candidate_path, f"fixed public target bytes are not pinned: {target_path}")
            plans.append((name, expected_identity, mapping))

        # All entries are validated before any unlink. Revalidate each name immediately before
        # removing it so unknown/raced content remains available at the reported path.
        for name, expected_identity, mapping in plans:
            candidate_path = os.path.join(staging_path, name)
            fd = open_stable_regular(staging_fd, name, expected_identity, candidate_path)
            try:
                if mapping is not None and digest_fd(fd) != mapping["sha256"]:
                    preserve(candidate_path, "published staging bytes changed before recovery")
            finally:
                os.close(fd)
            if mapping is not None:
                target_path = mapping["target"]
                target_name = os.path.basename(target_path)
                target_metadata = os.stat(target_name, dir_fd=parent_fd, follow_symlinks=False)
                if identity(target_metadata) != expected_identity:
                    preserve(candidate_path, f"fixed public target changed before recovery: {target_path}")
                target_fd = open_stable_regular(
                    parent_fd, target_name, expected_identity, target_path
                )
                try:
                    if digest_fd(target_fd) != mapping["sha256"]:
                        preserve(candidate_path, f"fixed public target bytes changed: {target_path}")
                finally:
                    os.close(target_fd)
                # The target hard-link may have been created immediately before SIGKILL and not
                # yet synced by the interrupted helper. Make publication durable before unlinking
                # the only recovery link.
                os.fsync(parent_fd)
            try:
                os.unlink(name, dir_fd=staging_fd)
            except OSError as error:
                preserve(candidate_path, f"cannot unlink validated staging recovery file: {error}")
        os.fsync(staging_fd)

        remaining = sorted(os.listdir(staging_fd))
        if remaining:
            preserve(os.path.join(staging_path, remaining[0]), "reserved staging namespace is not empty")
        current_stage = os.stat(staging_name, dir_fd=parent_fd, follow_symlinks=False)
        if identity(current_stage) != identity(os.fstat(staging_fd)):
            preserve(staging_path, "reserved staging namespace changed before retirement")
        try:
            os.rmdir(staging_name, dir_fd=parent_fd)
        except OSError as error:
            preserve(staging_path, f"cannot retire empty reserved staging namespace: {error}")
        os.fsync(parent_fd)
    finally:
        if staging_fd is not None:
            os.close(staging_fd)
        os.close(parent_fd)


try:
    main()
except RecoveryError as error:
    print(f"pinvou-megabook-profile: {error}", file=sys.stderr)
    raise SystemExit(1)
except (OSError, ValueError) as error:
    path = sys.argv[3] if len(sys.argv) > 3 else "fixed reserved staging namespace"
    print(
        f"pinvou-megabook-profile: staging recovery failed: {error}; "
        f"preserved recovery path: {path}",
        file=sys.stderr,
    )
    raise SystemExit(1)
PY
}

cleanup_staging_orphans_impl() {
  validate_existing_fixed_directory_chains || return 1
  cleanup_reserved_staging_set \
    "$profile_staging_dir" "$profile_dir" profile 600:644 "$RESERVED_STAGING_ALLOWED_LINKS" \
    644 "$PROFILE_BYTES" "$PROFILE_SHA256" "$profile_target" \
    || return 1
  cleanup_reserved_staging_set \
    "$desktop_staging_dir" "$desktop_dir" desktop 600:644 "$RESERVED_STAGING_ALLOWED_LINKS" \
    644 "$DESKTOP_BYTES" "$DESKTOP_SHA256" "$desktop_target" \
    || return 1
  cleanup_reserved_staging_set \
    "$marker_staging_dir" "$state_dir" marker 600 "$RESERVED_STAGING_ALLOWED_LINKS" \
    600 "$LEGACY_MARKER_BYTES" "$LEGACY_MARKER_SHA256" "$legacy_marker_target" \
    600 "$INSTALLING_MARKER_BYTES" "$INSTALLING_MARKER_SHA256" "$installing_marker_target" \
    600 "$APPLIED_MARKER_BYTES" "$APPLIED_MARKER_SHA256" "$applied_marker_target" \
    || return 1
}

cleanup_staging_orphans() {
  cleanup_staging_orphans_impl \
    || fail "reserved staging recovery failed; the preserved recovery path is reported above"
}

cleanup_on_exit() {
  saved_status=$?
  trap - 0
  if [ "$cleanup_trap_ready" -eq 1 ]; then
    cleanup_staging_orphans_impl || saved_status=1
  fi
  exit "$saved_status"
}

cleanup_trap_ready=1
trap cleanup_on_exit 0
trap 'exit 1' HUP INT TERM

write_marker() {
  phase=$1
  marker_file=$2
  /usr/bin/printf '%s\n' \
    'schema=pinvou-megabook-profile-v2' \
    "phase=$phase" \
    "profile_sha256=$PROFILE_SHA256" \
    "desktop_sha256=$DESKTOP_SHA256" >"$marker_file"
}

publish_marker_no_clobber() {
  phase=$1
  marker_target=$2
  expected_sha=$3
  marker_chain_identity=$(fixed_parent_chain_identity "$state_dir") \
    || fail "cannot validate the fixed marker directory chain"
  prepare_private_staging_directory "$marker_staging_dir" "$state_dir"
  marker_tmp=$(/usr/bin/mktemp "$marker_staging_dir/marker.XXXXXX") \
    || fail "cannot stage the $phase ownership marker"
  write_marker "$phase" "$marker_tmp" || fail "cannot write the ownership marker"
  validate_owned_file "$marker_tmp" 600 "$expected_sha"
  fsync_file "$marker_tmp"
  [ "$marker_chain_identity" = "$(fixed_parent_chain_identity "$state_dir")" ] \
    || fail "fixed marker directory chain changed before publication"
  /usr/bin/ln -T -- "$marker_tmp" "$marker_target" \
    || fail "$phase ownership marker appeared concurrently; refusing to overwrite it"
  fsync_directory "$state_dir"
  [ "$marker_chain_identity" = "$(fixed_parent_chain_identity "$state_dir")" ] \
    || fail "fixed marker directory chain changed during publication"
  validate_owned_file_one_of "$marker_tmp" 600 2 "$expected_sha"
  validate_fixed_public_file_one_of "$marker_target" 600 2 "$expected_sha"
  [ "$(file_identity "$marker_tmp")" = "$(file_identity "$marker_target")" ] \
    || fail "published marker is not the staged inode"
  /usr/bin/rm -- "$marker_tmp" || fail "cannot retire marker staging link"
  fsync_directory "$marker_staging_dir"
  cleanup_reserved_staging_set \
    "$marker_staging_dir" "$state_dir" marker 600 "$RESERVED_STAGING_ALLOWED_LINKS" \
    600 "$LEGACY_MARKER_BYTES" "$LEGACY_MARKER_SHA256" "$legacy_marker_target" \
    600 "$INSTALLING_MARKER_BYTES" "$INSTALLING_MARKER_SHA256" "$installing_marker_target" \
    600 "$APPLIED_MARKER_BYTES" "$APPLIED_MARKER_SHA256" "$applied_marker_target" \
    || fail "cannot retire the empty marker staging namespace: $marker_staging_dir"
  validate_fixed_public_file "$marker_target" 600 "$expected_sha"
}

install_source_no_clobber() {
  install_source_file=$1
  install_target_file=$2
  install_target_directory=$3
  install_temporary_prefix=$4
  install_expected_sha=$5
  case "$install_temporary_prefix" in
    .pinvou-profile)
      install_staging_directory=$profile_staging_dir
      install_staging_prefix=profile
      install_expected_bytes=$PROFILE_BYTES
      ;;
    .pinvou-desktop)
      install_staging_directory=$desktop_staging_dir
      install_staging_prefix=desktop
      install_expected_bytes=$DESKTOP_BYTES
      ;;
    *) fail "internal temporary prefix is not fixed" ;;
  esac
  install_chain_identity=$(fixed_parent_chain_identity "$install_target_directory") \
    || fail "cannot validate fixed registered-target directory chain: $install_target_directory"
  prepare_private_staging_directory \
    "$install_staging_directory" "$install_target_directory"
  install_temporary=$(/usr/bin/mktemp \
    "$install_staging_directory/$install_staging_prefix.XXXXXX") \
    || fail "cannot stage registered target: $install_target_file"
  /usr/bin/install -m 0644 -- "$install_source_file" "$install_temporary" \
    || fail "cannot populate registered target: $install_target_file"
  validate_owned_file "$install_temporary" 644 "$install_expected_sha"
  fsync_file "$install_temporary"
  [ "$install_chain_identity" \
      = "$(fixed_parent_chain_identity "$install_target_directory")" ] \
    || fail "fixed registered-target directory chain changed before publication"
  /usr/bin/ln -T -- "$install_temporary" "$install_target_file" \
    || fail "registered target appeared concurrently; refusing to overwrite it: $install_target_file"
  fsync_directory "$install_target_directory"
  [ "$install_chain_identity" \
      = "$(fixed_parent_chain_identity "$install_target_directory")" ] \
    || fail "fixed registered-target directory chain changed during publication"
  validate_owned_file_one_of "$install_temporary" 644 2 "$install_expected_sha"
  validate_fixed_public_file_one_of \
    "$install_target_file" 644 2 "$install_expected_sha"
  [ "$(file_identity "$install_temporary")" = "$(file_identity "$install_target_file")" ] \
    || fail "published target is not the staged inode: $install_target_file"
  /usr/bin/rm -- "$install_temporary" \
    || fail "cannot retire target staging link: $install_target_file"
  fsync_directory "$install_staging_directory"
  cleanup_reserved_staging_set \
    "$install_staging_directory" "$install_target_directory" \
    "$install_staging_prefix" 600:644 \
    "$RESERVED_STAGING_ALLOWED_LINKS" \
    644 "$install_expected_bytes" "$install_expected_sha" "$install_target_file" \
    || fail "cannot retire the empty target staging namespace: $install_staging_directory"
  validate_fixed_public_file "$install_target_file" 644 "$install_expected_sha"
}

finish_residual_quarantine() {
  target=$1
  quarantine=$2
  expected_mode=$3
  expected_sha=$4
  directory=$5
  validate_existing_fixed_directory_chains
  if [ ! -e "$quarantine" ] && [ ! -L "$quarantine" ]; then
    return 0
  fi
  residual_chain_identity=$(fixed_parent_chain_identity "$directory") \
    || fail "cannot validate interrupted-removal directory chain: $directory"
  validate_fixed_public_file "$quarantine" "$expected_mode" "$expected_sha"
  if [ -e "$target" ] || [ -L "$target" ]; then
    fail "interrupted removal conflicts with a new target; preserved quarantine: $quarantine"
  fi
  /usr/bin/rm -- "$quarantine" || fail "cannot retire validated quarantine: $quarantine"
  fsync_directory "$directory"
  [ "$residual_chain_identity" = "$(fixed_parent_chain_identity "$directory")" ] \
    || fail "fixed directory chain changed while retiring quarantine: $directory"
}

quarantine_and_delete() {
  target=$1
  quarantine=$2
  expected_mode=$3
  expected_sha=$4
  directory=$5
  finish_residual_quarantine "$target" "$quarantine" "$expected_mode" "$expected_sha" "$directory"
  if [ ! -e "$target" ] && [ ! -L "$target" ]; then
    return 0
  fi
  removal_chain_identity=$(fixed_parent_chain_identity "$directory") \
    || fail "cannot validate registered-target removal directory chain: $directory"
  validate_fixed_public_file "$target" "$expected_mode" "$expected_sha"
  captured_identity=$(file_identity "$target") || fail "cannot identify removal target: $target"
  [ ! -e "$quarantine" ] && [ ! -L "$quarantine" ] \
    || fail "quarantine appeared concurrently: $quarantine"
  /usr/bin/mv -T -n -- "$target" "$quarantine" \
    || fail "cannot atomically quarantine registered target: $target"
  fsync_directory "$directory"
  [ "$removal_chain_identity" = "$(fixed_parent_chain_identity "$directory")" ] \
    || fail "fixed directory chain changed while quarantining target: $directory"
  if [ -e "$target" ] || [ -L "$target" ] \
      || [ ! -e "$quarantine" ] || [ -L "$quarantine" ] \
      || [ "$(file_identity "$quarantine" 2>/dev/null || true)" != "$captured_identity" ]; then
    fail "registered target raced with quarantine; preserved recovery path: $quarantine"
  fi
  # Validation failure exits with the exact quarantine path and leaves it untouched. Editors that
  # atomic-save the public target can no longer make this helper unlink their replacement.
  validate_fixed_public_file "$quarantine" "$expected_mode" "$expected_sha"
  if [ -e "$target" ] || [ -L "$target" ]; then
    fail "a replacement appeared after quarantine; preserved original at: $quarantine"
  fi
  /usr/bin/rm -- "$quarantine" || fail "cannot delete validated quarantine: $quarantine"
  fsync_directory "$directory"
  [ "$removal_chain_identity" = "$(fixed_parent_chain_identity "$directory")" ] \
    || fail "fixed directory chain changed while deleting quarantine: $directory"
  [ ! -e "$target" ] && [ ! -L "$target" ] \
    || fail "a replacement appeared while retiring quarantine: $target"
}

require_no_quarantines() {
  validate_existing_fixed_directory_chains
  for quarantine in \
    "$profile_quarantine" "$desktop_quarantine" \
    "$legacy_marker_quarantine" "$installing_marker_quarantine" \
    "$applied_marker_quarantine"; do
    [ ! -e "$quarantine" ] && [ ! -L "$quarantine" ] \
      || fail "an interrupted profile transaction requires activate/deactivate recovery: $quarantine"
  done
}

clear_activation_quarantines() {
  finish_residual_quarantine \
    "$profile_target" "$profile_quarantine" 644 "$PROFILE_SHA256" "$profile_dir"
  finish_residual_quarantine \
    "$desktop_target" "$desktop_quarantine" 644 "$DESKTOP_SHA256" "$desktop_dir"
  finish_residual_quarantine \
    "$legacy_marker_target" "$legacy_marker_quarantine" 600 \
    "$LEGACY_MARKER_SHA256" "$state_dir"
  finish_residual_quarantine \
    "$installing_marker_target" "$installing_marker_quarantine" 600 \
    "$INSTALLING_MARKER_SHA256" "$state_dir"
  finish_residual_quarantine \
    "$applied_marker_target" "$applied_marker_quarantine" 600 \
    "$APPLIED_MARKER_SHA256" "$state_dir"
}

daemon_reload() {
  /usr/bin/systemctl --user daemon-reload \
    || fail "the user systemd manager rejected daemon-reload"
}

require_app_inactive() {
  active_state=$(/usr/bin/systemctl --user show "$APP_UNIT" --property=ActiveState --value) \
    || fail "cannot inspect the fixed app unit"
  main_pid=$(/usr/bin/systemctl --user show "$APP_UNIT" --property=MainPID --value) \
    || fail "cannot inspect the fixed app MainPID"
  case "$active_state:$main_pid" in
    inactive:0|failed:0) ;;
    *) fail "stop pinvou3-app.service before changing its resource profile" ;;
  esac
}

validate_fixed_supervisor() {
  [ ! -L "$SUPERVISOR" ] && [ -f "$SUPERVISOR" ] && [ -x "$SUPERVISOR" ] \
    || fail "fixed Supervisor is missing, linked, or not executable: $SUPERVISOR"
  supervisor_metadata=$(/usr/bin/stat -c %d:%i:%u:%g:%a:%h:%s -- "$SUPERVISOR") \
    || fail "cannot inspect the fixed Supervisor: $SUPERVISOR"
  supervisor_size=${supervisor_metadata##*:}
  supervisor_metadata_without_size=${supervisor_metadata%:*}
  case "$supervisor_size" in
    ''|*[!0-9]*|0) fail "fixed Supervisor is empty: $SUPERVISOR" ;;
  esac
  case "$supervisor_metadata_without_size" in
    *:*:0:0:755:1) ;;
    *) fail "fixed Supervisor must be root:root regular non-symlink 0755 nlink1: $SUPERVISOR" ;;
  esac
  [ ! -L "$SUPERVISOR" ] \
    && [ "$supervisor_metadata" = "$(/usr/bin/stat -c %d:%i:%u:%g:%a:%h:%s -- "$SUPERVISOR")" ] \
    || fail "fixed Supervisor changed during metadata validation: $SUPERVISOR"
  /usr/bin/printf '%s\n' "$supervisor_metadata"
}

validate_supervisor_status_receipt() {
  trusted_supervisor_identity=$(validate_fixed_supervisor) \
    || fail "fixed Supervisor metadata validation failed"
  status_receipt=$("$SUPERVISOR" status) \
    || fail "fixed Supervisor status request did not return a receipt"
  [ "$trusted_supervisor_identity" = "$(validate_fixed_supervisor)" ] \
    || fail "fixed Supervisor changed while obtaining its status receipt"
  /usr/bin/printf '%s' "$status_receipt" | /usr/bin/python3 -I -c '
import json
import re
import sys


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON member: " + key)
        result[key] = value
    return result


raw = sys.stdin.buffer.read(32769)
if not raw or len(raw) > 32768:
    raise SystemExit("Supervisor receipt is empty or exceeds the protocol bound")
try:
    receipt = json.loads(raw.decode("utf-8"), object_pairs_hook=unique_object)
except (UnicodeDecodeError, json.JSONDecodeError, ValueError) as error:
    raise SystemExit("Supervisor receipt is not unique canonical JSON: " + str(error))
expected_receipt_keys = {
    "protocol_version",
    "request_id",
    "target",
    "descriptor_revision",
    "expected_instance_generation",
    "action",
    "outcome",
    "observation",
    "detail",
    "observed_at_unix_ms",
}
if not isinstance(receipt, dict) or set(receipt) != expected_receipt_keys:
    raise SystemExit("Supervisor receipt schema is not protocol v2")
request_id = receipt.get("request_id")
if not isinstance(request_id, str) \
        or not request_id.startswith("status:") \
        or not (1 <= len(request_id.encode("utf-8")) <= 128) \
        or re.fullmatch(r"[A-Za-z0-9_.:-]+", request_id) is None:
    raise SystemExit("Supervisor status request_id is invalid")
if receipt.get("protocol_version") != 2 \
        or receipt.get("target") != "pinvou_app" \
        or receipt.get("descriptor_revision") != "pinvou-app-descriptor-v1" \
        or receipt.get("expected_instance_generation") is not None \
        or receipt.get("action") != "status" \
        or receipt.get("outcome") != "reconciled":
    raise SystemExit("Supervisor receipt does not confirm the fixed protocol-v2 status request")
observation = receipt.get("observation")
expected_observation_keys = {
    "instance_generation",
    "state",
    "sub_state",
    "unit_result",
    "main_pid",
    "restart_count",
    "cgroup",
}
if not isinstance(observation, dict) or set(observation) != expected_observation_keys:
    raise SystemExit("Supervisor receipt has no protocol-v2 app observation")
state = observation.get("state")
generation = observation.get("instance_generation")
if state not in ("inactive", "failed") or observation.get("main_pid") is not None:
    raise SystemExit("Supervisor did not observe the fixed app stopped with null MainPID")
if state == "inactive" and generation is not None:
    raise SystemExit("inactive Supervisor observation retained an instance generation")
if state == "failed" and generation is not None \
        and (not isinstance(generation, str) \
             or re.fullmatch(r"[0-9a-f]{32}", generation) is None \
             or generation == "0" * 32):
    raise SystemExit("failed Supervisor observation has an invalid residual generation")
' || fail "fixed Supervisor did not return a trusted stopped-app status receipt"
}

validate_effective_profile() {
  validate_fixed_parent_chain "$profile_dir"
  trusted_supervisor_identity=$(validate_fixed_supervisor) \
    || fail "fixed Supervisor metadata validation failed"
  effective_properties=$(/usr/bin/systemctl --user show "$APP_UNIT" --no-pager \
    --property=LoadState,FragmentPath,DropInPaths,MemoryAccounting,MemoryHigh,MemoryMax,MemorySwapMax,OOMPolicy,KillMode,TasksMax,Restart,RestartUSec,StartLimitIntervalUSec,StartLimitBurst,Environment) \
    || fail "cannot inspect the complete effective app profile"
  /usr/bin/printf '%s\n' "$effective_properties" | /usr/bin/python3 -I -c '
import re
import shlex
import sys


raw = sys.stdin.read()
if len(raw.encode("utf-8")) > 65536:
    raise SystemExit("effective property receipt exceeds its fixed bound")
properties = {}
for line in raw.splitlines():
    if "=" not in line:
        raise SystemExit("effective property receipt contains a malformed line")
    key, value = line.split("=", 1)
    if key in properties:
        raise SystemExit("effective property receipt contains a duplicate: " + key)
    properties[key] = value
expected_keys = {
    "LoadState",
    "FragmentPath",
    "DropInPaths",
    "MemoryAccounting",
    "MemoryHigh",
    "MemoryMax",
    "MemorySwapMax",
    "OOMPolicy",
    "KillMode",
    "TasksMax",
    "Restart",
    "RestartUSec",
    "StartLimitIntervalUSec",
    "StartLimitBurst",
    "Environment",
}
if set(properties) != expected_keys:
    raise SystemExit("effective property receipt is incomplete or contains unknown fields")


def duration_usec(value):
    multipliers = {
        "us": 1,
        "µs": 1,
        "μs": 1,
        "ms": 1000,
        "s": 1000_000,
        "min": 60 * 1000_000,
        "h": 60 * 60 * 1000_000,
        "d": 24 * 60 * 60 * 1000_000,
        "w": 7 * 24 * 60 * 60 * 1000_000,
        "month": 30 * 24 * 60 * 60 * 1000_000,
        "y": 365 * 24 * 60 * 60 * 1000_000,
    }
    if not value or value == "infinity":
        return None
    total = 0
    for component in value.split():
        match = re.fullmatch(r"([0-9]+)(us|µs|μs|ms|s|min|h|d|w|month|y)?", component)
        if match is None:
            return None
        amount = int(match.group(1))
        suffix = match.group(2)
        if suffix is None:
            if amount != 0:
                return None
            multiplier = 1
        else:
            multiplier = multipliers[suffix]
        total += amount * multiplier
    return total


fixed = {
    "LoadState": "loaded",
    "FragmentPath": sys.argv[2],
    "MemoryAccounting": "yes",
    "MemoryHigh": "4294967296",
    "MemoryMax": "8589934592",
    "MemorySwapMax": "2147483648",
    "OOMPolicy": "kill",
    "KillMode": "control-group",
    "TasksMax": "512",
    "Restart": "on-failure",
    "StartLimitBurst": "3",
}
for key, expected in fixed.items():
    if properties.get(key) != expected:
        raise SystemExit(f"effective {key} does not match the fixed profile")
try:
    dropins = shlex.split(properties["DropInPaths"], posix=True)
except ValueError as error:
    raise SystemExit("effective DropInPaths is malformed: " + str(error))
if dropins != [sys.argv[1]]:
    raise SystemExit("effective DropInPaths is not exactly the registered profile")
if duration_usec(properties["RestartUSec"]) != 15_000_000:
    raise SystemExit("effective RestartUSec is not 15 seconds")
if duration_usec(properties["StartLimitIntervalUSec"]) != 300_000_000:
    raise SystemExit("effective StartLimitIntervalUSec is not 300 seconds")
try:
    environment_tokens = shlex.split(properties["Environment"], posix=True)
except ValueError as error:
    raise SystemExit("effective Environment is malformed: " + str(error))
environment = {}
for token in environment_tokens:
    if "=" not in token:
        raise SystemExit("effective Environment contains a non-assignment")
    key, value = token.split("=", 1)
    if not key or key in environment:
        raise SystemExit("effective Environment contains an empty or duplicate name")
    environment[key] = value
if environment != {
    "PINVOU_SUPERVISED": "1",
    "PINVOU_RESOURCE_PROFILE": "megabook-canary-v1",
}:
    raise SystemExit("effective Environment is not the exact fixed identity set")
' "$profile_target" "$APP_FRAGMENT" \
    || fail "effective app profile failed the complete fixed-property validation"
  [ "$trusted_supervisor_identity" = "$(validate_fixed_supervisor)" ] \
    || fail "fixed Supervisor changed before status receipt validation"
  validate_supervisor_status_receipt
}

sources_preflight() {
  validate_source "$PROFILE_SOURCE" "$PROFILE_SHA256" "$PROFILE_BYTES"
  validate_source "$DESKTOP_SOURCE" "$DESKTOP_SHA256" "$DESKTOP_BYTES"
  validate_existing_fixed_directory_chains
}

detect_registered_state() {
  validate_existing_fixed_directory_chains
  registration_kind=inactive
  legacy_present=0
  installing_present=0
  applied_present=0
  if [ -e "$legacy_marker_target" ] || [ -L "$legacy_marker_target" ]; then
    validate_fixed_public_file "$legacy_marker_target" 600 "$LEGACY_MARKER_SHA256"
    legacy_present=1
  fi
  if [ -e "$installing_marker_target" ] || [ -L "$installing_marker_target" ]; then
    validate_fixed_public_file \
      "$installing_marker_target" 600 "$INSTALLING_MARKER_SHA256"
    installing_present=1
  fi
  if [ -e "$applied_marker_target" ] || [ -L "$applied_marker_target" ]; then
    validate_fixed_public_file "$applied_marker_target" 600 "$APPLIED_MARKER_SHA256"
    applied_present=1
  fi
  if [ "$legacy_present" -eq 1 ] && { [ "$installing_present" -eq 1 ] || [ "$applied_present" -eq 1 ]; }; then
    fail "legacy and v2 profile markers coexist; refusing an ambiguous upgrade"
  fi
  if [ "$legacy_present" -eq 1 ]; then
    registration_kind=legacy
  elif [ "$installing_present" -eq 1 ] && [ "$applied_present" -eq 1 ]; then
    registration_kind=v2-transition
  elif [ "$installing_present" -eq 1 ]; then
    registration_kind=v2-installing
  elif [ "$applied_present" -eq 1 ]; then
    registration_kind=v2-applied
  elif [ -e "$profile_target" ] || [ -L "$profile_target" ] \
      || [ -e "$desktop_target" ] || [ -L "$desktop_target" ]; then
    fail "unregistered target exists; refusing to claim or remove it"
  fi
}

validate_or_install_targets() {
  validate_fixed_parent_chain "$profile_dir"
  validate_fixed_parent_chain "$desktop_dir"
  if [ -e "$profile_target" ] || [ -L "$profile_target" ]; then
    validate_fixed_public_file "$profile_target" 644 "$PROFILE_SHA256"
  else
    install_source_no_clobber \
      "$PROFILE_SOURCE" "$profile_target" "$profile_dir" .pinvou-profile "$PROFILE_SHA256"
  fi
  if [ -e "$desktop_target" ] || [ -L "$desktop_target" ]; then
    validate_fixed_public_file "$desktop_target" 644 "$DESKTOP_SHA256"
  else
    install_source_no_clobber \
      "$DESKTOP_SOURCE" "$desktop_target" "$desktop_dir" .pinvou-desktop "$DESKTOP_SHA256"
  fi
}

status_profile() {
  sources_preflight
  cleanup_staging_orphans
  require_no_quarantines
  detect_registered_state
  case "$registration_kind" in
    inactive)
      /usr/bin/printf '%s\n' inactive
      ;;
    v2-installing|v2-transition)
      fail "profile activation is incomplete; run activate or deactivate to recover"
      ;;
    legacy|v2-applied)
      validate_fixed_public_file "$profile_target" 644 "$PROFILE_SHA256"
      validate_fixed_public_file "$desktop_target" 644 "$DESKTOP_SHA256"
      validate_effective_profile
      /usr/bin/printf '%s\n' active
      ;;
    *) fail "internal registration state is invalid" ;;
  esac
}

activate_profile() {
  sources_preflight
  require_app_inactive
  ensure_owned_directory "$home_dir/.config"
  ensure_owned_directory "$home_dir/.config/systemd"
  ensure_owned_directory "$home_dir/.config/systemd/user"
  ensure_owned_directory "$profile_dir"
  ensure_owned_directory "$home_dir/.local"
  ensure_owned_directory "$home_dir/.local/share"
  ensure_owned_directory "$desktop_dir"
  ensure_owned_directory "$home_dir/.local/state"
  ensure_owned_directory "$state_dir"
  cleanup_staging_orphans
  clear_activation_quarantines
  detect_registered_state

  case "$registration_kind" in
    inactive)
      publish_marker_no_clobber installing \
        "$installing_marker_target" "$INSTALLING_MARKER_SHA256"
      registration_kind=v2-installing
      ;;
    legacy|v2-installing|v2-applied|v2-transition) ;;
    *) fail "internal registration state is invalid" ;;
  esac
  validate_or_install_targets
  daemon_reload
  require_app_inactive
  validate_effective_profile

  case "$registration_kind" in
    legacy|v2-applied) ;;
    v2-installing)
      publish_marker_no_clobber applied "$applied_marker_target" "$APPLIED_MARKER_SHA256"
      quarantine_and_delete \
        "$installing_marker_target" "$installing_marker_quarantine" 600 \
        "$INSTALLING_MARKER_SHA256" "$state_dir"
      ;;
    v2-transition)
      quarantine_and_delete \
        "$installing_marker_target" "$installing_marker_quarantine" 600 \
        "$INSTALLING_MARKER_SHA256" "$state_dir"
      ;;
  esac
  status_profile
}

deactivate_profile() {
  sources_preflight
  cleanup_staging_orphans
  require_app_inactive
  finish_residual_quarantine \
    "$profile_target" "$profile_quarantine" 644 "$PROFILE_SHA256" "$profile_dir"
  finish_residual_quarantine \
    "$desktop_target" "$desktop_quarantine" 644 "$DESKTOP_SHA256" "$desktop_dir"
  finish_residual_quarantine \
    "$legacy_marker_target" "$legacy_marker_quarantine" 600 \
    "$LEGACY_MARKER_SHA256" "$state_dir"
  finish_residual_quarantine \
    "$installing_marker_target" "$installing_marker_quarantine" 600 \
    "$INSTALLING_MARKER_SHA256" "$state_dir"
  finish_residual_quarantine \
    "$applied_marker_target" "$applied_marker_quarantine" 600 \
    "$APPLIED_MARKER_SHA256" "$state_dir"
  detect_registered_state
  if [ "$registration_kind" = inactive ]; then
    /usr/bin/printf '%s\n' inactive
    return 0
  fi

  if [ -e "$profile_target" ] || [ -L "$profile_target" ]; then
    validate_fixed_public_file "$profile_target" 644 "$PROFILE_SHA256"
  fi
  if [ -e "$desktop_target" ] || [ -L "$desktop_target" ]; then
    validate_fixed_public_file "$desktop_target" 644 "$DESKTOP_SHA256"
  fi
  require_app_inactive
  quarantine_and_delete \
    "$profile_target" "$profile_quarantine" 644 "$PROFILE_SHA256" "$profile_dir"
  quarantine_and_delete \
    "$desktop_target" "$desktop_quarantine" 644 "$DESKTOP_SHA256" "$desktop_dir"
  daemon_reload
  require_app_inactive
  if [ -e "$applied_marker_target" ] || [ -L "$applied_marker_target" ]; then
    quarantine_and_delete \
      "$applied_marker_target" "$applied_marker_quarantine" 600 \
      "$APPLIED_MARKER_SHA256" "$state_dir"
  fi
  if [ -e "$installing_marker_target" ] || [ -L "$installing_marker_target" ]; then
    quarantine_and_delete \
      "$installing_marker_target" "$installing_marker_quarantine" 600 \
      "$INSTALLING_MARKER_SHA256" "$state_dir"
  fi
  if [ -e "$legacy_marker_target" ] || [ -L "$legacy_marker_target" ]; then
    quarantine_and_delete \
      "$legacy_marker_target" "$legacy_marker_quarantine" 600 \
      "$LEGACY_MARKER_SHA256" "$state_dir"
  fi
  /usr/bin/printf '%s\n' inactive
}

[ "$#" -eq 1 ] || fail "usage: pinvou-megabook-profile <activate|deactivate|status>"
case "$1" in
  activate) activate_profile ;;
  deactivate) deactivate_profile ;;
  status) status_profile ;;
  *) fail "usage: pinvou-megabook-profile <activate|deactivate|status>" ;;
esac
