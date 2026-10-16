#!/usr/bin/env bash
# cache-janitor.sh — GitHub Actions 缓存清理(10GB 配额保护)
#
# 删除规则:
#   A. 已关闭/已合并 PR(refs/pull/N/merge)作用域的全部缓存
#      (PR 作用域缓存互不可见,PR 关闭后即死重;PR 状态查询失败一律保留)
#   B. codeql-overlay 每种语言每个作用域只保留最新 1 份(CodeQL 只消费最新 base)
#   C. node-cache 每个平台每个作用域只保留最新 1 份(同平台旧 lockfile 哈希
#      仅有 restore-keys 前缀回退价值,留最新即可)
#   D. v0-rust-* keeps only the newest generation per series (shared-key) per
#      scope (new in 2026-10: rust-cache's key embeds the lockfile hash, so
#      every dependency-surface change mints a new generation, and older
#      generations have no restore-fallback value — prefix fallback on the
#      PR/queue side always hits the newest generation).
#      The two rust-lint-v2 generations alone held 4.2GB ≈ 42% of the quota.
#      The old "main-scoped v0-rust-* whole-series protection" is demoted to
#      a "dedup": quota measured a standing 10.7/10GB on 2026-10-01 and LRU
#      was evicting the most valuable windows/macos caches; without this
#      rule, LRU alone would pick the eviction victim.
#      A series is split off at the first platform marker
#      (-Linux-/-Darwin-/-Windows_NT-), so series names exclude the platform
#      segment: reusing the same shared-key across platforms/architectures
#      would dedupe across groups (ubuntu-22.04 and ubuntu-22.04-arm both
#      produce the -Linux- marker and differ only in the arch segment; every
#      current shared-key is single-platform single-arch — extend the
#      grouping with platform and arch before any cross reuse).
#
# Note: rules B/C/D must group by cache scope (ref). Scopes are invisible to
# each other; keeping only the newest across refs would swap a main-usable
# cache for a PR-scoped one main cannot read — effectively a wrong delete.
#
# 用法:scripts/cache-janitor.sh [--dry-run]
# 依赖:gh cli;python3;GH_TOKEN 需具备 actions:write(删除)与 pull-requests:read(查状态)。
set -euo pipefail

DRY_RUN=0
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY_RUN=1 ;;
    *) echo "未知参数: $1" >&2; exit 2 ;;
  esac
  shift
done

# 删除计数(按规则分组统计,便于审计日志;不用关联数组以兼容 macOS bash 3.2)
STAT_A_CLOSED_PR=0
STAT_B_CODEQL=0
STAT_C_NODE=0
STAT_D_RUST_GEN=0
DELETED_BYTES=0

delete_cache() { # $1=id $2=key $3=size_bytes $4=rule
  local id="$1" key="$2" size="$3" rule="$4"
  if [ "$DRY_RUN" -eq 1 ]; then
    echo "[dry-run][$rule] 将删除 #$id ($(( size / 1048576 ))MB) $key"
  else
    echo "[$rule] 删除 #$id ($(( size / 1048576 ))MB) $key"
    if ! gh cache delete "$id"; then
      echo "::warning::删除失败 #$id $key(可能已被并发驱逐)"
      return 0
    fi
  fi
  # 仅在删除成功后计数(dry-run 为预估),避免删除失败仍计入"已释放"导致审计虚高
  case "$rule" in
    A_closed_pr)  STAT_A_CLOSED_PR=$(( STAT_A_CLOSED_PR + 1 )) ;;
    B_codeql)     STAT_B_CODEQL=$(( STAT_B_CODEQL + 1 )) ;;
    C_node)       STAT_C_NODE=$(( STAT_C_NODE + 1 )) ;;
    D_rust_gen)   STAT_D_RUST_GEN=$(( STAT_D_RUST_GEN + 1 )) ;;
  esac
  DELETED_BYTES=$(( DELETED_BYTES + size ))
}

echo "== 枚举缓存(dry-run=$DRY_RUN) =="
gh cache list --limit 5000 --json id,key,ref,createdAt,lastAccessedAt,sizeInBytes > /tmp/cache-janitor-list.json
TOTAL=$(python3 -c "import json; d=json.load(open('/tmp/cache-janitor-list.json')); print(len(d))")
echo "共 $TOTAL 个缓存条目"

# ---------- 规则 A:已关闭/合并 PR 的全部缓存 ----------
echo "== 规则 A:已关闭/合并 PR 的缓存 =="
CLOSED_REFS=""   # 规则 B/C 需跳过这些 ref(已由本规则处理,避免误删保留项)
PR_REFS=$(python3 -c "
import json
refs = {c['ref'] for c in json.load(open('/tmp/cache-janitor-list.json')) if c['ref'].startswith('refs/pull/')}
for r in sorted(refs): print(r)
")
for ref in $PR_REFS; do
  pr_num="${ref#refs/pull/}"; pr_num="${pr_num%%/*}"
  state=$(gh pr view "$pr_num" --json state --jq '.state' 2>/dev/null || echo "UNKNOWN")
  # 只删确认已关闭/合并的;查询失败(UNKNOWN)一律保留,避免 API 抖动误删开放 PR 的缓存
  if [ "$state" != "CLOSED" ] && [ "$state" != "MERGED" ]; then
    echo "  PR #$pr_num 状态=$state,保留 $ref"
    continue
  fi
  echo "  PR #$pr_num 状态=$state,清理 $ref"
  CLOSED_REFS="$CLOSED_REFS $ref"
  while IFS=$'\t' read -r id size key; do
    delete_cache "$id" "$key" "$size" "A_closed_pr"
  done < <(python3 -c "
import json
for c in json.load(open('/tmp/cache-janitor-list.json')):
    if c['ref'] == '$ref': print(f\"{c['id']}\t{c['sizeInBytes']}\t{c['key']}\")
")
done
export CLOSED_REFS

# ---------- 规则 B:codeql-overlay 每语言留最新 1 份 ----------
echo "== 规则 B:codeql-overlay 去重 =="
while IFS=$'\t' read -r id size key; do
  delete_cache "$id" "$key" "$size" "B_codeql"
done < <(python3 -c "
import json, os, re
closed = set(os.environ.get('CLOSED_REFS','').split())
entries = [c for c in json.load(open('/tmp/cache-janitor-list.json')) if c['key'].startswith('codeql-overlay-base-database-') and c['ref'] not in closed]
groups = {}
for c in entries:
    # key 形如 codeql-overlay-base-database-1-<sha>-<lang>-<version>-...
    # lang 可能含连字符(如 javascript-typescript),取 sha 之后、版本号段
    # (x.y.z)之前的全部片段,避免截断成 javascript 造成分组键语义错误。
    # 按 (语言, 作用域) 分组:缓存作用域互不可见,跨 ref 只留最新会把 main 的
    # 缓存换成 PR 作用域的(main 读不到),等于删掉 main 的可用缓存。
    parts = c['key'].split('-')
    lang_parts = []
    for p in parts[6:]:
        if re.match(r'^\d+\.\d+', p): break
        lang_parts.append(p)
    lang = '-'.join(lang_parts) if lang_parts else c['key']
    groups.setdefault((lang, c['ref']), []).append(c)
for group, items in groups.items():
    items.sort(key=lambda c: c['createdAt'], reverse=True)
    for c in items[1:]: print(f\"{c['id']}\t{c['sizeInBytes']}\t{c['key']}\")
")

# ---------- 规则 C:node-cache 每平台留最新 1 份 ----------
echo "== 规则 C:node-cache 去重 =="
while IFS=$'\t' read -r id size key; do
  delete_cache "$id" "$key" "$size" "C_node"
done < <(python3 -c "
import json, os
closed = set(os.environ.get('CLOSED_REFS','').split())
entries = [c for c in json.load(open('/tmp/cache-janitor-list.json')) if c['key'].startswith('node-cache-') and c['ref'] not in closed]
groups = {}
for c in entries:
    # key 形如 node-cache-<platform>-npm-<hash>,平台段取 npm- 之前的部分
    # 按 (平台, 作用域) 分组:同规则 B,跨 ref 只留最新会误删 main 的可用缓存。
    platform = c['key'].split('-npm-')[0]
    groups.setdefault((platform, c['ref']), []).append(c)
for group, items in groups.items():
    items.sort(key=lambda c: c['createdAt'], reverse=True)
    for c in items[1:]: print(f\"{c['id']}\t{c['sizeInBytes']}\t{c['key']}\")
")

# ---------- Rule D: keep only the newest generation per v0-rust-* series ----------
echo "== Rule D: v0-rust-* generation dedup =="
while IFS=$'\t' read -r id size key; do
  delete_cache "$id" "$key" "$size" "D_rust_gen"
done < <(python3 -c "
import json, os
closed = set(os.environ.get('CLOSED_REFS','').split())
entries = [c for c in json.load(open('/tmp/cache-janitor-list.json')) if c['key'].startswith('v0-rust-') and c['ref'] not in closed]
groups = {}
for c in entries:
    # Entries without createdAt cannot be ordered; exclude them instead of
    # letting the sort below raise TypeError — that would silently kill all
    # of rule D (nobody checks the process substitution's failure status),
    # stalling the dedup for weeks.
    if not c.get('createdAt'):
        continue
    # Keys look like v0-rust-<shared-key>-<platform>-<arch>-<hashes...> with
    # platform ∈ {Linux, Darwin, Windows_NT}. The series name is the
    # shared-key segment: everything after v0-rust- and before the first
    # platform marker (e.g. rust-lint-v2 / windows-rust-test /
    # release-linux-x64-jammy). Group by (series, scope) and keep the newest
    # createdAt generation per group; older generations are worthless for
    # restore-keys prefix fallback (it always hits the newest) — pure quota
    # weight.
    rest = c['key'][len('v0-rust-'):]
    series = rest
    for marker in ('-Linux-', '-Darwin-', '-Windows_NT-'):
        idx = rest.find(marker)
        if idx != -1:
            series = rest[:idx]
            break
    groups.setdefault((series, c['ref']), []).append(c)
for group, items in groups.items():
    items.sort(key=lambda c: c['createdAt'], reverse=True)
    for c in items[1:]: print(f\"{c['id']}\t{c['sizeInBytes']}\t{c['key']}\")
")

echo "== 汇总 =="
echo "A_closed_pr=$STAT_A_CLOSED_PR B_codeql=$STAT_B_CODEQL C_node=$STAT_C_NODE D_rust_gen=$STAT_D_RUST_GEN"
echo "释放空间约 $(( DELETED_BYTES / 1048576 ))MB(dry-run=$DRY_RUN)"
