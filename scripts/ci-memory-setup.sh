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
# Idempotent and best-effort on zram: if the kernel lacks the zram module or
# lz4, the script warns loudly and continues with the disk swap only, so a
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

setup_zram() {
  if ! modprobe zram 2>/dev/null; then
    warn "kernel zram module not loadable; continuing with disk swap only"
    return 0
  fi
  local sysfs=/sys/block/zram0 dev=/dev/zram0
  if [[ ! -e ${sysfs}/disksize ]]; then
    warn "no zram0 device after modprobe; continuing with disk swap only"
    return 0
  fi
  if [[ $(<"${sysfs}/disksize") -ne 0 ]]; then
    warn "zram0 already in use; leaving it untouched"
    return 0
  fi
  # The compressor can only be selected while disksize is still 0.
  if ! echo lz4 >"${sysfs}/comp_algorithm"; then
    warn "lz4 not accepted by zram; continuing with disk swap only"
    return 0
  fi
  echo "${ZRAM_SIZE_BYTES}" >"${sysfs}/disksize"
  mkswap "${dev}" >/dev/null
  swapon -p "${ZRAM_PRIORITY}" "${dev}"
  log "zram ready: ${dev} lz4 ${ZRAM_SIZE_BYTES} bytes, priority ${ZRAM_PRIORITY}"
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

# swappiness >100 requires kernel >= 5.8; all supported runner images qualify.
sysctl -w vm.swappiness=130 >/dev/null
sysctl -w vm.watermark_scale_factor=300 >/dev/null
sysctl -w vm.min_free_kbytes=65536 >/dev/null
sysctl -w vm.overcommit_memory=1 >/dev/null
log "sysctl tuned: swappiness=130 watermark_scale_factor=300 min_free_kbytes=65536 overcommit_memory=1"

setup_zram
setup_disk_swap

log "final swap layout:"
swapon --show
zramctl || true
