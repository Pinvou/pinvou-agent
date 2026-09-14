#!/usr/bin/env bash
# CI Linux runner memory setup.
#
# Standard GitHub-hosted Linux runners provide 16 GiB RAM and only a 4 GiB
# /mnt/swapfile. The pinvou3 workspace (700+ crates, ThinLTO, dep-level O2)
# repeatedly exhausts that budget in rust-test; before this script existed the
# failure mode was "hosted runner lost communication" with all logs lost.
#
# This script enlarges effective memory at job start (every Linux job runs it
# right after checkout):
#   - 24 GiB lz4 zram device, swap priority 100 (used first; compressed pages
#     never touch disk, so lld/rustc cold pages cost CPU instead of disk I/O);
#   - 16 GiB /mnt/swapfile (replaces the image's 4 GiB one), priority 10,
#     overflowing only after zram fills;
#   - kernel tunables: vm.swappiness=130 (>=100 shifts reclaim towards
#     anonymous pages, i.e. actually use the swap we just built),
#     vm.watermark_scale_factor=300 and vm.min_free_kbytes=65536 (wake kswapd
#     earlier and keep a deeper emergency reserve so the runner agent's
#     heartbeat survives link peaks), vm.overcommit_memory=1 (never refuse
#     mmap; let swap absorb peaks instead of failing allocations).
#
# The hosted images often ship without /lib/modules matching the running
# kernel, so modprobe zram fails there. The script then installs the matching
# modules package and retries once; if the kernel has no loadable zram at all
# it falls back to zswap (built-in), which compresses swapped pages with lz4
# into a RAM pool in front of the disk swap — the same "compress in RAM first,
# spill to disk second" effect. Every degradation is loud but non-fatal, so a
# runner-image change can never break unrelated jobs.

set -euo pipefail

log() { echo "[memory-setup] $*"; }
warn() { echo "[memory-setup] WARNING: $*" >&2; }

if [[ ${EUID} -ne 0 ]]; then
  warn "must run as root (invoke as: sudo bash scripts/ci-memory-setup.sh)"
  exit 1
fi

ZRAM_SIZE_BYTES=$((24 * 1024 * 1024 * 1024))
DISK_SWAP_SIZE=16G
ZRAM_PRIORITY=100
DISK_SWAP_PRIORITY=10

ensure_zram_module() {
  local moderr
  if modprobe zram 2>/dev/null; then
    return 0
  fi
  moderr="$(modprobe zram 2>&1 || true)"
  warn "modprobe zram failed (${moderr}); installing matching kernel modules"
  if command -v apt-get >/dev/null 2>&1; then
    apt-get update -qq >/dev/null 2>&1 || true
    apt-get install -y -qq --no-install-recommends \
      "linux-modules-extra-$(uname -r)" >/dev/null 2>&1 || true
  fi
  if modprobe zram 2>/dev/null; then
    log "zram module loaded after installing linux-modules-extra-$(uname -r)"
    return 0
  fi
  return 1
}

setup_zram() {
  if ! ensure_zram_module; then
    return 1
  fi
  local sysfs=/sys/block/zram0 dev=/dev/zram0
  if [[ ! -e ${sysfs}/disksize ]]; then
    warn "no zram0 device after modprobe; skipping zram"
    return 1
  fi
  if [[ $(<"${sysfs}/disksize") -ne 0 ]]; then
    warn "zram0 already in use; leaving it untouched"
    return 0
  fi
  # The compressor can only be selected while disksize is still 0.
  if ! echo lz4 >"${sysfs}/comp_algorithm"; then
    warn "lz4 not accepted by zram; skipping zram"
    return 1
  fi
  echo "${ZRAM_SIZE_BYTES}" >"${sysfs}/disksize"
  mkswap "${dev}" >/dev/null
  swapon -p "${ZRAM_PRIORITY}" "${dev}"
  log "zram ready: ${dev} lz4 ${ZRAM_SIZE_BYTES} bytes, priority ${ZRAM_PRIORITY}"
  return 0
}

setup_disk_swap() {
  # The runner image pre-creates a 4 GiB /mnt/swapfile via waagent; replace it.
  if swapon --show=NAME --noheadings 2>/dev/null | grep -qx '/mnt/swapfile'; then
    swapoff /mnt/swapfile
  fi
  rm -f /mnt/swapfile
  # /mnt is the ephemeral local SSD with ~80 GiB free; fallocate keeps the
  # swapfile non-sparse as required by swapon.
  fallocate -l "${DISK_SWAP_SIZE}" /mnt/swapfile
  chmod 600 /mnt/swapfile
  mkswap /mnt/swapfile >/dev/null
  swapon -p "${DISK_SWAP_PRIORITY}" /mnt/swapfile
  log "disk swap ready: /mnt/swapfile ${DISK_SWAP_SIZE}, priority ${DISK_SWAP_PRIORITY}"
}

# zram and zswap overlap: zswap is only enabled when the zram device could not
# be created, so the "compress in RAM first" layer exists exactly once.
setup_zswap_fallback() {
  local params=/sys/module/zswap/params
  if [[ ! -d ${params} ]]; then
    warn "zswap unavailable too; continuing with plain disk swap"
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
  log "zswap fallback enabled: compressed RAM pool (25% of RAM) in front of disk swap"
}

# swappiness >100 requires kernel >= 5.8; all supported runner images qualify.
sysctl -w vm.swappiness=130 >/dev/null
sysctl -w vm.watermark_scale_factor=300 >/dev/null
sysctl -w vm.min_free_kbytes=65536 >/dev/null
sysctl -w vm.overcommit_memory=1 >/dev/null
log "sysctl tuned: swappiness=130 watermark_scale_factor=300 min_free_kbytes=65536 overcommit_memory=1"

setup_disk_swap
if ! setup_zram; then
  setup_zswap_fallback
fi

log "final swap layout:"
swapon --show
zramctl || true
if [[ -d /sys/module/zswap/params ]]; then
  grep -H . /sys/module/zswap/params/enabled /sys/module/zswap/params/compressor \
    /sys/module/zswap/params/max_pool_percent || true
fi
