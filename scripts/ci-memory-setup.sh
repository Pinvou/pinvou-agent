#!/usr/bin/env bash
# CI Linux runner memory setup (best effort, never fatal).
#
# Facts measured on 2026-09-15 with probes on all three hosted images
# (ubuntu-22.04, ubuntu-24.04, ubuntu-22.04-arm):
#   - Hosted runners are single-disk: / and /mnt are the same ext4
#     (/dev/sda1). x64 images total 72G with only ~13-14 GiB free at boot,
#     arm ~34 GiB free. A swapfile on /mnt therefore eats the build disk
#     directly, so disk swap is no longer created by default.
#   - The private-repo hosted runner is 2-core / 7.8 GiB RAM (free -h);
#     the "16 GiB RAM" claimed by older comments was the public-repo spec.
#
# Swap layers, preferred first:
#   1. zram (default ON): 24 GiB lz4, priority 100; compressed pages never
#      touch disk, and the compressed pool is capped at 50% of RAM
#      (mem_limit) so an incompressible workload cannot eat the runner's
#      whole memory through zram itself. Re-qualified on all three images
#      via modprobe zram ->
#      (on failure: activate the image swapfile first if nothing is active,
#      then apt-get install linux-modules-extra-$(uname -r), then modprobe
#      again) -> lz4 -> 24G disksize -> mem_limit -> mkswap -> swapon -p 100.
#      The 2026-09-12 hosted job hangs once blamed on zram (run
#      34708283784) are re-classified as a transient environment incident:
#      the same script ran green with zram loaded on the community repo the
#      same day. The countermeasure is structural, not a rollback: every
#      userspace external call (modprobe, apt-get, fallocate, mkswap,
#      swapon, swapoff) runs under `timeout` with a SIGKILL backstop, and
#      any failure degrades loudly to the next layer. Residual risk that no
#      userspace measure can remove: an in-kernel hang (module load or a
#      stuck sysfs write) is uninterruptible; the workflow-side
#      `timeout 240` + non-fatal wrapper is the last line there, and
#      PINVOU3_CI_DISABLE_ZRAM=1 is the standing opt-out.
#   2. Opt-in /mnt/swapfile (priority 10), only with
#      PINVOU3_CI_ENABLE_DISK_SWAP=1, for self-hosted runners where /mnt is
#      a real second disk. The default path never fallocates and never
#      swapoff/rm's the image-provided swap (/swapfile, ~3G, left as is).
#   3. Image swapfile as last resort: only when the runner came up with
#      zero active swap (probe data: some ubuntu-22.04 boots ship /swapfile
#      but leave it inactive), activate it so a zram failure degrades to
#      "plain swap" instead of "7.8 GiB RAM and nothing else".
#   4. zswap in front of whatever swap remains, enabled only when no
#      /dev/zram swap is active, so the "compress in RAM first" layer
#      exists exactly once.
#
# Environment switches:
#   PINVOU3_CI_DISABLE_ZRAM=1      skip zram entirely (explicit opt-out for
#                                  future incidents). Replaces the old
#                                  PINVOU3_CI_ENABLE_ZRAM opt-in; zram is
#                                  now default-on.
#   PINVOU3_CI_ENABLE_DISK_SWAP=1  additionally create /mnt/swapfile.
#
# Kernel tunables (each knob independent, failure only warns):
#   vm.swappiness=130 (>=100 shifts reclaim towards anonymous pages, i.e.
#   actually use the swap layers above), vm.watermark_scale_factor=300 and
#   vm.min_free_kbytes=65536 (wake kswapd earlier and keep a deeper
#   emergency reserve so the runner agent's heartbeat survives link peaks),
#   vm.overcommit_memory=1 (never refuse mmap; let swap absorb peaks
#   instead of failing allocations).
#
# The pinvou3 workspace (700+ crates, ThinLTO, dep-level O2) repeatedly
# exhausts the stock memory budget in rust-test; before memory provisioning
# existed the failure mode was "hosted runner lost communication" with all
# logs lost. Every Linux job that needs extra memory runs this script right
# after checkout (enforced by the gate policy; rust-lint is exempt — its
# lint-only workload fits in stock memory).

set -uo pipefail

log() { echo "[memory-setup] $*"; }
warn() { echo "[memory-setup] WARNING: $*" >&2; }

if [[ ${EUID} -ne 0 ]]; then
  warn "must run as root (invoke as: sudo bash scripts/ci-memory-setup.sh)"
  exit 0
fi

# run_to SECS CMD [ARGS...]: run CMD under a hard timeout (TERM, then
# SIGKILL after 15s) so a hung userspace call can never outlive its cap;
# without coreutils timeout, run CMD bare (every call site stays non-fatal
# either way). A call stuck in an uninterruptible kernel state cannot be
# killed by any userspace measure — that residual is documented in the
# header above.
run_to() {
  local secs=$1
  shift
  if command -v timeout >/dev/null 2>&1; then
    timeout -k 15 "${secs}" "$@"
  else
    "$@"
  fi
}

# True when at least one swap device is currently active.
any_swap_active() {
  swapon --show=NAME --noheadings 2>/dev/null | grep -q .
}

# sysctl tuning: each knob is independent and non-fatal.
sysctl -w vm.swappiness=130 >/dev/null 2>&1 \
  || warn "vm.swappiness=130 rejected by the running kernel"
sysctl -w vm.watermark_scale_factor=300 >/dev/null 2>&1 \
  || warn "vm.watermark_scale_factor=300 rejected"
sysctl -w vm.min_free_kbytes=65536 >/dev/null 2>&1 \
  || warn "vm.min_free_kbytes=65536 rejected"
sysctl -w vm.overcommit_memory=1 >/dev/null 2>&1 \
  || warn "vm.overcommit_memory=1 rejected"
log "sysctl tuned (best effort): swappiness=130 watermark_scale_factor=300 min_free_kbytes=65536 overcommit_memory=1"

ZRAM_SIZE_BYTES=$((24 * 1024 * 1024 * 1024))
ZRAM_PRIORITY=100
DISK_SWAP_SIZE_KIB=$((16 * 1024 * 1024))
DISK_SWAP_PRIORITY=10

setup_zram() {
  # Module load first; on hosted images the zram module may live in the
  # linux-modules-extra package, so install it and retry once before
  # giving up. Every external call is timeout-capped, with its stderr kept
  # in the warning so the actual failure reason survives in the log.
  local modprobe_err
  if ! modprobe_err="$(run_to 60 modprobe zram 2>&1)"; then
    warn "modprobe zram failed${modprobe_err:+: ${modprobe_err}}; installing linux-modules-extra-$(uname -r) and retrying"
    # The module-install path (apt-get, up to minutes) is the slowest stretch
    # of this script and the only one the workflow-side `timeout 240` can
    # realistically interrupt. Activate the image swapfile BEFORE it, so a
    # mid-apt kill degrades to "plain swap" and never to zero swap; if zram
    # comes up afterwards it simply takes over as the higher-priority layer.
    if ! any_swap_active; then
      warn "no swap active; activating the image swapfile before the slow module install"
      activate_image_swap_fallback
    fi
    if ! run_to 120 apt-get update -qq; then
      warn "apt-get update failed; the module install below may fail too"
    fi
    if ! run_to 120 apt-get install -y -qq --no-install-recommends \
      "linux-modules-extra-$(uname -r)"; then
      warn "apt-get install linux-modules-extra-$(uname -r) failed"
    fi
    if ! modprobe_err="$(run_to 60 modprobe zram 2>&1)"; then
      warn "modprobe zram still failing after module install${modprobe_err:+: ${modprobe_err}}; giving up on zram"
      return 1
    fi
  fi

  local size_file=/sys/block/zram0/disksize
  if [[ ! -e ${size_file} ]]; then
    warn "/sys/block/zram0/disksize not found; giving up on zram"
    return 1
  fi
  local current_size
  current_size="$(cat "${size_file}" 2>/dev/null || echo 0)"
  if [[ ${current_size} != 0 ]]; then
    # disksize non-zero only counts as "already in use" when a swap on
    # /dev/zram0 is actually active; a stale disksize from a killed earlier
    # run would otherwise report zram ready while providing no swap.
    if swapon --show=NAME --noheadings 2>/dev/null | grep -qx '/dev/zram0'; then
      log "zram0 already in use (disksize=${current_size} bytes, swap active); leaving it untouched"
      return 0
    fi
    warn "zram0 has stale disksize=${current_size} bytes without an active swap; resetting it once"
    if ! echo 0 >"${size_file}" 2>/dev/null; then
      warn "could not reset the stale zram0 disksize; giving up on zram"
      return 1
    fi
  fi

  # Configure zram0: compressor -> size -> pool cap -> mkswap -> swapon.
  # Each step warns instead of aborting, except the pool cap below, which
  # fails closed: an uncapped zram on a 7.8 GiB runner is worse than no
  # zram. Only an inactive zram swap device counts as overall failure so
  # the next layer takes over.
  if echo lz4 >/sys/block/zram0/comp_algorithm 2>/dev/null; then
    log "zram0 compressor set to lz4"
  else
    warn "could not set zram0 compressor to lz4; keeping the kernel default"
  fi
  if ! echo "${ZRAM_SIZE_BYTES}" >"${size_file}" 2>/dev/null; then
    warn "could not set zram0 disksize; giving up on zram"
    return 1
  fi
  log "zram0 disksize set to 24 GiB"

  # Cap the compressed pool at 50% of RAM. disksize is only the virtual
  # capacity; the pool grows with stored pages and lz4 keeps incompressible
  # pages near 1:1, so an unbounded pool on a 7.8 GiB runner could eat all
  # RAM through zram itself and reproduce the "runner lost communication"
  # failure this script exists to prevent. Writes beyond mem_limit fail the
  # swap write and surface as ordinary memory pressure instead. Fail closed:
  # if MemTotal cannot be read or the mem_limit write fails, reset the device
  # and return failure so the fallback layers engage; never activate an
  # uncapped zram.
  local mem_total_kib mem_limit_bytes
  mem_total_kib="$(awk '/^MemTotal:/ {print $2}' /proc/meminfo 2>/dev/null || echo 0)"
  if [[ ${mem_total_kib} =~ ^[0-9]+$ ]] && ((mem_total_kib > 0)); then
    mem_limit_bytes=$((mem_total_kib * 1024 / 2))
    if ! echo "${mem_limit_bytes}" >/sys/block/zram0/mem_limit 2>/dev/null; then
      warn "could not set zram0 mem_limit; resetting zram0 so the fallback swap layers engage"
      echo 0 >"${size_file}" 2>/dev/null \
        || warn "could not reset the zram0 disksize; giving up on zram"
      return 1
    fi
    log "zram0 pool capped at $((mem_limit_bytes / 1024 / 1024)) MiB (50% of RAM)"
  else
    warn "cannot read MemTotal; resetting zram0 so the fallback swap layers engage"
    echo 0 >"${size_file}" 2>/dev/null \
      || warn "could not reset the zram0 disksize; giving up on zram"
    return 1
  fi

  # udev/devtmpfs usually creates the node synchronously, but wait briefly
  # so a fresh boot does not degrade on a node-creation race.
  local waits=0
  while [[ ! -b /dev/zram0 && ${waits} -lt 10 ]]; do
    sleep 0.5
    waits=$((waits + 1))
  done
  [[ -b /dev/zram0 ]] || warn "/dev/zram0 node still absent after waiting; mkswap/swapon below may fail"

  local mkswap_err
  if ! mkswap_err="$(run_to 60 mkswap /dev/zram0 2>&1)"; then
    warn "mkswap /dev/zram0 failed${mkswap_err:+: ${mkswap_err}}; swapon will most likely fail too"
  fi
  local swapon_err
  if ! swapon_err="$(run_to 60 swapon -p "${ZRAM_PRIORITY}" /dev/zram0 2>&1)"; then
    warn "swapon /dev/zram0 failed${swapon_err:+: ${swapon_err}}"
    return 1
  fi
  log "zram swap active: /dev/zram0 24 GiB lz4, priority ${ZRAM_PRIORITY}"
  return 0
}

# Opt-in disk swap (PINVOU3_CI_ENABLE_DISK_SWAP=1) for self-hosted runners
# where /mnt is a real second disk. Never runs on hosted runners: there /
# and /mnt share one ext4, so this file would shrink the build disk.
setup_disk_swap() {
  # /mnt safety checks are load bearing: on images where /mnt is tmpfs
  # (RAM backed) or has less free space than the requested swapfile,
  # fallocate would exhaust memory instantly and take the runner agent
  # down with the job. Size to at most 60% of the actual free space and
  # skip RAM-backed mounts entirely.
  local fstype avail_kib cap_kib want_kib
  fstype="$(findmnt -n -o FSTYPE /mnt 2>/dev/null || true)"
  if [[ ${fstype} == tmpfs ]]; then
    warn "/mnt is tmpfs (RAM backed); skipping disk swap"
    return 0
  fi
  avail_kib="$(df -kP /mnt 2>/dev/null | awk 'NR==2 {print $4}')"
  [[ ${avail_kib} =~ ^[0-9]+$ ]] || { warn "cannot determine /mnt free space; skipping disk swap"; return 0; }
  want_kib=${DISK_SWAP_SIZE_KIB}
  cap_kib=$((avail_kib * 60 / 100))
  if (( want_kib > cap_kib )); then
    want_kib=${cap_kib}
  fi
  if (( want_kib < 1024 * 1024 )); then
    warn "/mnt free space too small for a swapfile; skipping disk swap"
    return 0
  fi
  if swapon --show=NAME --noheadings 2>/dev/null | grep -qx '/mnt/swapfile'; then
    # swapoff of a large active swapfile can take minutes; the cap keeps it
    # bounded and on timeout the swap simply stays active (kept below).
    if run_to 120 swapoff /mnt/swapfile 2>/dev/null; then
      rm -f /mnt/swapfile
    else
      warn "could not swapoff the active /mnt/swapfile; keeping it as is instead of rebuilding"
      return 0
    fi
  fi
  if ! run_to 60 fallocate -l "${want_kib}K" /mnt/swapfile 2>/dev/null; then
    warn "fallocate ${want_kib}K /mnt/swapfile failed; keeping the existing swap configuration"
    return 0
  fi
  chmod 600 /mnt/swapfile
  run_to 60 mkswap /mnt/swapfile >/dev/null 2>&1 || { warn "mkswap failed"; return 0; }
  if run_to 60 swapon -p "${DISK_SWAP_PRIORITY}" /mnt/swapfile 2>/dev/null; then
    log "disk swap ready: /mnt/swapfile $((want_kib / 1024)) MiB, priority ${DISK_SWAP_PRIORITY}"
  else
    warn "swapon /mnt/swapfile failed; keeping the existing swap configuration"
  fi
}

# Last-resort layer: when the runner came up with zero active swap (zram
# unavailable and no opt-in disk swap), activate the image-provided
# swapfile so the fallback is "plain swap" and never "7.8 GiB RAM and
# nothing else". Probe data: some ubuntu-22.04 boots ship /swapfile but
# leave it inactive, so an existing file must not be assumed to be an
# active swap. Reformatting is attempted only when the file cannot be
# swapon'd as is (ephemeral runner, no active swap to lose).
activate_image_swap_fallback() {
  local cand
  for cand in /swapfile /mnt/swapfile; do
    [[ -f ${cand} ]] || continue
    if run_to 60 swapon "${cand}" 2>/dev/null; then
      log "last-resort swap active: ${cand} (image-provided)"
      return 0
    fi
    warn "swapon ${cand} failed as is; trying chmod 600 + mkswap + swapon once"
    chmod 600 "${cand}" 2>/dev/null || true
    if run_to 60 mkswap "${cand}" >/dev/null 2>&1 \
      && run_to 60 swapon "${cand}" 2>/dev/null; then
      log "last-resort swap active after reformat: ${cand}"
      return 0
    fi
    warn "could not activate ${cand} as swap"
  done
  warn "no swap layer could be activated; continuing with RAM only"
}

# zram and zswap overlap: zswap is only enabled when no /dev/zram swap is
# active, so the "compress in RAM first" layer exists exactly once.
setup_zswap_fallback() {
  local params=/sys/module/zswap/params
  if [[ ! -d ${params} ]]; then
    warn "zswap unavailable too; continuing with the existing swap configuration"
    return 0
  fi
  echo 1 >"${params}/enabled" 2>/dev/null || true
  if echo lz4 >"${params}/compressor" 2>/dev/null; then
    log "zswap compressor set to lz4"
  else
    warn "zswap rejected lz4; keeping kernel default compressor"
  fi
  echo zsmalloc >"${params}/zpool" 2>/dev/null || true
  echo 25 >"${params}/max_pool_percent" 2>/dev/null || true
  log "zswap fallback enabled: compressed RAM pool (25% of RAM) in front of the remaining swap"
}

if [[ ${PINVOU3_CI_DISABLE_ZRAM:-0} == 1 ]]; then
  log "PINVOU3_CI_DISABLE_ZRAM=1: skipping zram (explicit opt-out)"
elif setup_zram; then
  log "zram layer ready"
else
  warn "zram layer unavailable; continuing with the remaining swap layers"
fi

if [[ ${PINVOU3_CI_ENABLE_DISK_SWAP:-0} == 1 ]]; then
  log "PINVOU3_CI_ENABLE_DISK_SWAP=1: provisioning the opt-in /mnt disk swap"
  setup_disk_swap
else
  log "disk swap on /mnt skipped by default (hosted / and /mnt share one disk; set PINVOU3_CI_ENABLE_DISK_SWAP=1 on real dual-disk runners)"
fi

if ! swapon --show=NAME --noheadings 2>/dev/null | grep -q '/dev/zram'; then
  setup_zswap_fallback
fi

if ! any_swap_active; then
  warn "no active swap after all layers; trying the image-provided swapfile"
  activate_image_swap_fallback
fi

log "final swap layout:"
swapon --show || true
if command -v zramctl >/dev/null 2>&1; then
  zramctl || true
fi
log "disk usage after memory setup:"
df -h / || true
exit 0
