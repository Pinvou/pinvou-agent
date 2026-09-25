#!/usr/bin/env bash
# CI Linux runner memory setup (best effort, never fatal).
#
# Facts measured on 2026-09-15 with probes on all three hosted images
# (ubuntu-22.04, ubuntu-24.04, ubuntu-22.04-arm; probes run in the sibling
# repo Pinvou/pinvou3):
#   - Hosted runners are single-disk: / and /mnt are the same ext4
#     (/dev/sda1). x64 images total 72G; with the 16G /mnt/swapfile an
#     earlier version of this script created, the build had only ~13-14
#     GiB of disk left — the direct cause of the release ENOSPC failures —
#     and arm ~34 GiB. A swapfile on /mnt therefore eats the build disk
#     directly, so disk swap is no longer created by default.
#   - The Pinvou/pinvou3 hosted runner is 2-core / 7.8 GiB RAM (free -h);
#     the "16 GiB RAM" claimed by older comments was the public-repo
#     runner spec.
#
# Swap layers, preferred first:
#   1. zram: zstd, priority 100; the virtual device is sized to 2x RAM at
#      runtime (from MemTotal), compressed pages never touch disk, and the
#      compressed pool is capped at 70% of RAM (mem_limit) so an
#      incompressible workload cannot eat the runner's whole memory through
#      zram itself. zram was already the unconditional first layer before
#      this tuning; the 2026-09-19 round only changes the compressor
#      (lz4 -> zstd), the device size (fixed 24 GiB -> 2x RAM) and the pool
#      cap (50% -> 70% of RAM). Qualified on all three hosted images by the
#      Pinvou/pinvou3 probes via modprobe zram ->
#      (on failure: activate the image swapfile first if nothing is active,
#      then apt-get install linux-modules-extra-$(uname -r), then modprobe
#      again) -> zstd -> 2x-RAM disksize -> mem_limit -> mkswap ->
#      swapon -p 100.
#      The 2026-09-12 hosted job hangs once blamed on zram (Pinvou/pinvou3
#      run 34708283784) were re-classified as a transient environment
#      incident: the same script ran green with zram loaded in this repo
#      the same day. The countermeasure is structural, not a rollback: every
#      userspace external call (modprobe, apt-get, fallocate, mkswap,
#      swapon, swapoff) runs under `timeout` with a SIGKILL backstop, and
#      any failure degrades loudly to the next layer. Residual risk that no
#      userspace measure can remove: an in-kernel hang (module load or a
#      stuck sysfs write) is uninterruptible; the workflow-side
#      `timeout 240` + non-fatal wrapper is the last line there, and
#      PINVOU3_CI_DISABLE_ZRAM=1 is the standing opt-out.
#   2. /mnt/swapfile (priority 10), 8 GiB, provisioned on EVERY runner:
#      swap is mandatory, not opt-in (2026-09-19 decision) — with the zram
#      pool hard-capped at 70% of RAM, this is the only unbounded overflow
#      layer. The safety guards stay: /mnt is skipped when tmpfs (RAM
#      backed) or too small, the file is capped at 60% of actual free
#      space, and an image-provided /mnt/swapfile is rebuilt in place. On
#      the single-disk hosted images this costs 8G of build disk — a
#      deliberate trade for the memory ceiling.
#   3. Image swapfile as last resort: only when the runner came up with
#      zero active swap (probe data: some ubuntu-22.04 boots ship /swapfile
#      but leave it inactive), activate it so a zram or disk-swap failure
#      degrades to "plain swap" instead of "7.8 GiB RAM and nothing else".
#   4. zswap in front of whatever swap remains: enabled only when no
#      /dev/zram swap is active, and disabled again when zram is active
#      (stock Ubuntu kernels ship zswap enabled by default), so the
#      "compress in RAM first" layer exists exactly once.
#
# Environment switches (they survive the workflow wrappers'
# `sudo --preserve-env` alongside GITHUB_ACTIONS; sudo env_reset would
# otherwise strip them from the job env, silently no-op'ing the opt-out):
#   PINVOU3_CI_DISABLE_ZRAM=1      skip zram entirely (explicit opt-out for
#                                  future incidents; zram has been the
#                                  default first layer since this script
#                                  was introduced). There is no disk-swap
#                                  switch: the /mnt swapfile is mandatory.
#
# Capacity trade in the default configuration: the zram pool stays
# hard-capped (an uncapped pool is the plausible mechanism of the
# 2026-09-12 "runner lost communication" incident, and an OOM-kill that
# leaves logs beats a lost runner). The 2026-09-19 round resized the disk
# swap to 8G and made it mandatory again (it was an unconditional 16G
# file until the single-disk ENOSPC findings made it opt-in): zram at
# 2x RAM is the primary absorber and the swapfile is the unbounded
# overflow beyond the 70% pool cap, at the price of 8G build disk on the
# single-disk hosted images.
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
# logs lost. Every Linux job runs this script right after checkout
# (enforced by the gate policy test, which requires the wrapped, non-fatal
# call on every ubuntu job — rust-lint included; its lint-only workload
# would fit in stock memory, but the uniform wrapper keeps the policy
# checkable).

set -uo pipefail

log() { echo "[memory-setup] $*"; }
# Surface degradation warnings as GitHub step annotations when running in
# Actions: the workflow-side wrapper only annotates the outer 240s timeout,
# so an in-script degradation would otherwise be step-log-only. Every call
# site passes GITHUB_ACTIONS through sudo (--preserve-env) so this branch
# actually engages; outside Actions (manual or self-hosted debugging runs)
# keep plain stderr text.
if [[ ${GITHUB_ACTIONS:-} == true ]]; then
  # Workflow commands are single-line: captured stderr (multi-line tool
  # output) must be folded or the annotation is cut at the first newline
  # (a stray CR splits lines too -- the runner's .NET line reader treats
  # it as a terminator, which could forge a second workflow command).
  warn() { echo "::warning::[memory-setup] ${*//[$'\r\n']/ }" >&2; }
else
  warn() { echo "[memory-setup] WARNING: $*" >&2; }
fi

# Steps that normally recover a few lines later (first modprobe miss on
# hosted images, the image-swap pre-activation belonging to that same
# recovery, the as-is swapon retry, and the last-resort image-swapfile
# attempt after all layers fail) must not burn a standing ::warning
# annotation on every job; keep them visible in the log only.
# Unrecovered failures still use warn.
warn_recoverable() { echo "[memory-setup] WARNING: $*" >&2; }

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

# zram knobs: the virtual device is ZRAM_RAM_MULT x RAM (derived at runtime
# from MemTotal) and the compressed pool is capped at ZRAM_POOL_RAM_PCT
# percent of RAM. The opt-in disk swap keeps a fixed size.
ZRAM_RAM_MULT=2
ZRAM_POOL_RAM_PCT=70
ZRAM_PRIORITY=100
DISK_SWAP_SIZE_KIB=$((8 * 1024 * 1024))
DISK_SWAP_PRIORITY=10

setup_zram() {
  # Module load first; on hosted images the zram module may live in the
  # linux-modules-extra package, so install it and retry once before
  # giving up. Every external call is timeout-capped, with its stderr kept
  # in the warning so the actual failure reason survives in the log.
  local modprobe_err
  if ! modprobe_err="$(run_to 60 modprobe zram 2>&1)"; then
    warn_recoverable "modprobe zram failed${modprobe_err:+: ${modprobe_err}}; installing linux-modules-extra-$(uname -r) and retrying"
    # The module-install path (apt-get, up to minutes) is the slowest stretch
    # of this script and the only one the workflow-side `timeout 240` can
    # realistically interrupt. Activate the image swapfile BEFORE it, so a
    # mid-apt kill degrades to "plain swap" and never to zero swap; if zram
    # comes up afterwards it simply takes over as the higher-priority layer.
    if ! any_swap_active; then
      warn_recoverable "no swap active; activating the image swapfile before the slow module install"
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

  # MemTotal drives both zram knobs (disksize = ZRAM_RAM_MULT x RAM, pool
  # cap = ZRAM_POOL_RAM_PCT% of RAM), so read it before touching the device
  # and fail closed when it cannot be read: the fallback swap layers engage
  # instead of an undersized or uncapped zram.
  local mem_total_kib zram_size_bytes zram_size_gib mem_limit_bytes
  mem_total_kib="$(awk '/^MemTotal:/ {print $2}' /proc/meminfo 2>/dev/null || echo 0)"
  if ! [[ ${mem_total_kib} =~ ^[0-9]+$ ]] || ((mem_total_kib <= 0)); then
    warn "cannot read MemTotal; giving up on zram so the fallback swap layers engage"
    return 1
  fi
  zram_size_bytes=$((mem_total_kib * 1024 * ZRAM_RAM_MULT))
  mem_limit_bytes=$((mem_total_kib * 1024 * ZRAM_POOL_RAM_PCT / 100))
  zram_size_gib=$((zram_size_bytes / 1024 / 1024 / 1024))

  # Configure zram0: compressor -> size -> pool cap -> mkswap -> swapon.
  # Each step warns instead of aborting, except the pool cap below, which
  # fails closed: an uncapped zram on a 7.8 GiB runner is worse than no
  # zram. Only an inactive zram swap device counts as overall failure so
  # the next layer takes over.
  if echo zstd >/sys/block/zram0/comp_algorithm 2>/dev/null; then
    log "zram0 compressor set to zstd"
  else
    warn "could not set zram0 compressor to zstd; keeping the kernel default"
  fi
  if ! echo "${zram_size_bytes}" >"${size_file}" 2>/dev/null; then
    warn "could not set zram0 disksize; giving up on zram"
    return 1
  fi
  log "zram0 disksize set to ${zram_size_gib} GiB (${ZRAM_RAM_MULT}x RAM)"

  # Cap the compressed pool at 70% of RAM. disksize is only the virtual
  # capacity; the pool grows with stored pages and zstd keeps incompressible
  # pages near 1:1, so an unbounded pool could eat all RAM through zram
  # itself and reproduce the "runner lost communication" failure this script
  # exists to prevent. Writes beyond mem_limit fail the swap write and
  # surface as ordinary memory pressure instead. Fail closed: if the
  # mem_limit write fails, reset the device and return failure so the
  # fallback layers engage; never activate an uncapped zram.
  if ! echo "${mem_limit_bytes}" >/sys/block/zram0/mem_limit 2>/dev/null; then
    warn "could not set zram0 mem_limit; resetting zram0 so the fallback swap layers engage"
    echo 0 >"${size_file}" 2>/dev/null \
      || warn "could not reset the zram0 disksize; giving up on zram"
    return 1
  fi
  log "zram0 pool capped at $((mem_limit_bytes / 1024 / 1024)) MiB (${ZRAM_POOL_RAM_PCT}% of RAM)"

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
  log "zram swap active: /dev/zram0 ${zram_size_gib} GiB zstd, priority ${ZRAM_PRIORITY}"
  return 0
}

# Mandatory disk swap: the 8 GiB /mnt/swapfile is the unbounded overflow
# layer beyond the zram pool cap and is provisioned on every runner (no
# opt-in switch since 2026-09-19). On hosted images / and /mnt share one
# ext4, so the guards below keep the file from taking the runner down.
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
  # Rebuilding replaces an active swapfile: once swapoff+rm succeed, a
  # later failure must not claim the previous configuration was kept.
  local removed_note=""
  if swapon --show=NAME --noheadings 2>/dev/null | grep -qx '/mnt/swapfile'; then
    # swapoff of a large active swapfile can take minutes; the cap keeps it
    # bounded and on timeout the swap simply stays active (kept below).
    if run_to 120 swapoff /mnt/swapfile 2>/dev/null; then
      # Only claim removal in removed_note if it actually happened.
      if rm -f /mnt/swapfile; then
        removed_note=" (the previous /mnt/swapfile was removed)"
      fi
    else
      warn "could not swapoff the active /mnt/swapfile; keeping it as is instead of rebuilding"
      return 0
    fi
  fi
  local call_err
  if ! call_err="$(run_to 60 fallocate -l "${want_kib}K" /mnt/swapfile 2>&1)"; then
    warn "fallocate ${want_kib}K /mnt/swapfile failed${call_err:+: ${call_err}}; continuing without a rebuilt disk swap${removed_note}"
    return 0
  fi
  chmod 600 /mnt/swapfile
  if ! call_err="$(run_to 60 mkswap /mnt/swapfile 2>&1)"; then
    warn "mkswap /mnt/swapfile failed${call_err:+: ${call_err}}; continuing without a rebuilt disk swap${removed_note}"
    return 0
  fi
  if call_err="$(run_to 60 swapon -p "${DISK_SWAP_PRIORITY}" /mnt/swapfile 2>&1)"; then
    log "disk swap ready: /mnt/swapfile $((want_kib / 1024)) MiB, priority ${DISK_SWAP_PRIORITY}"
  else
    warn "swapon /mnt/swapfile failed${call_err:+: ${call_err}}; continuing without disk swap${removed_note}"
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
    warn_recoverable "swapon ${cand} failed as is; trying chmod 600 + mkswap + swapon once"
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
# active, and disabled again when zram is active (stock Ubuntu kernels ship
# zswap enabled by default, which would double-compress every swapped page
# in front of zram), so the "compress in RAM first" layer exists exactly
# once.
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

# Disk swap is mandatory (2026-09-19): the 8G /mnt swapfile is the only
# unbounded overflow beyond the 70%-capped zram pool, so no opt-in switch
# remains. setup_disk_swap's guards still skip RAM-backed or nearly-full
# /mnt instead of taking the runner agent down.
log "provisioning the mandatory /mnt disk swap"
setup_disk_swap

if swapon --show=NAME --noheadings 2>/dev/null | grep -q '/dev/zram'; then
  # Keep the "compress in RAM first" layer to exactly one: stock Ubuntu
  # kernels ship zswap enabled by default, and left on it would sit in
  # front of zram and compress every swapped page twice (once in zswap,
  # once again in zram).
  if [[ -d /sys/module/zswap/params ]]; then
    if echo 0 >/sys/module/zswap/params/enabled 2>/dev/null; then
      log "zswap disabled (zram is the single compress-in-RAM layer)"
    else
      warn "could not disable zswap; swapped pages may be compressed twice (zswap in front of zram)"
    fi
  fi
else
  setup_zswap_fallback
fi

if ! any_swap_active; then
  # The last-resort retry usually activates an image swapfile (success logs
  # its own line) and its terminal failure already annotates inside the
  # function; annotating here too would leave a standing marker on every
  # swapless job, against the recoverable-noise policy.
  warn_recoverable "no active swap after all layers; trying the image-provided swapfile"
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
