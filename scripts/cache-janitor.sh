#!/usr/bin/env bash
# cache-janitor.sh — GitHub Actions 缓存清理(10GB 配额保护)
#
# 删除规则:
#   A. 已关闭/已合并 PR(refs/pull/N/merge)作用域的全部缓存
#      (PR 作用域缓存互不可见,PR 关闭后即死重;历史上 node-cache、sccache、
#       release 打包缓存都在 PR 作用域产生过 GB 级残留;PR 状态查询失败一律保留)
#   B. 最后访问超过 N 天的 sccache 对象(默认 7 天;稳定依赖的编译对象可能
#      创建很早但持续命中,必须按 lastAccessedAt 判断热度,否则会每周误删
#      仍在使用的主线暖缓存;内容寻址,真陈旧的条目命中前早已被新产物替代)
#   C. codeql-overlay 每种语言每个作用域只保留最新 1 份(CodeQL 只消费最新 base)
#   D. node-cache 每个平台每个作用域只保留最新 1 份(同平台旧 lockfile 哈希
#      仅有 restore-keys 前缀回退价值,留最新即可)
#
# 注意:规则 C/D 必须按缓存作用域(ref)分组。作用域之间互不可见,跨 ref
# 只留最新会把 main 的可用缓存换成 PR 作用域的(main 读不到),等于误删。
#
# 保护:main 作用域的 v0-rust-* 系列(rust-test / rust-lint / windows-rust-test /
# mac-build / release-*)不在任何规则范围内。
#
# 用法:scripts/cache-janitor.sh [--dry-run] [--sccache-days N]
# 依赖:gh cli;GH_TOKEN 需具备 actions:write(删除)与 pull-requests:read(查状态)。
set -euo pipefail

DRY_RUN=0
SCCACHE_DAYS=7
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY_RUN=1 ;;
    --sccache-days) SCCACHE_DAYS="$2"; shift ;;
    *) echo "未知参数: $1" >&2; exit 2 ;;
  esac
  shift
done

# 删除计数(按规则分组统计,便于审计日志;不用关联数组以兼容 macOS bash 3.2)
STAT_A_CLOSED_PR=0
STAT_B_SCCACHE_STALE=0
STAT_C_CODEQL=0
STAT_D_NODE=0
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
    A_closed_pr)     STAT_A_CLOSED_PR=$(( STAT_A_CLOSED_PR + 1 )) ;;
    B_sccache_stale) STAT_B_SCCACHE_STALE=$(( STAT_B_SCCACHE_STALE + 1 )) ;;
    C_codeql)        STAT_C_CODEQL=$(( STAT_C_CODEQL + 1 )) ;;
    D_node)          STAT_D_NODE=$(( STAT_D_NODE + 1 )) ;;
  esac
  DELETED_BYTES=$(( DELETED_BYTES + size ))
}

NOW_EPOCH=$(date -u +%s)
SCCACHE_CUTOFF=$(( NOW_EPOCH - SCCACHE_DAYS * 86400 ))

echo "== 枚举缓存(dry-run=$DRY_RUN, sccache-days=$SCCACHE_DAYS) =="
gh cache list --limit 5000 --json id,key,ref,createdAt,lastAccessedAt,sizeInBytes > /tmp/cache-janitor-list.json
TOTAL=$(python3 -c "import json; d=json.load(open('/tmp/cache-janitor-list.json')); print(len(d))")
echo "共 $TOTAL 个缓存条目"

# ---------- 规则 A:已关闭/合并 PR 的全部缓存 ----------
echo "== 规则 A:已关闭/合并 PR 的缓存 =="
CLOSED_REFS=""   # 规则 B/C/D 需跳过这些 ref(已由本规则处理,避免误删保留项)
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

# ---------- 规则 B:陈旧 sccache 对象 ----------
echo "== 规则 B:sccache 条目(最后访问超过 ${SCCACHE_DAYS} 天) =="
while IFS=$'\t' read -r id size key; do
  delete_cache "$id" "$key" "$size" "B_sccache_stale"
done < <(python3 -c "
import json, datetime, os
closed = set(os.environ.get('CLOSED_REFS','').split())
cutoff = $SCCACHE_CUTOFF
def ts(c):
    # 按最后访问时间判断热度:稳定依赖的对象创建很早但持续命中;
    # lastAccessedAt 缺失时回退 createdAt
    raw = c.get('lastAccessedAt') or c['createdAt']
    return datetime.datetime.fromisoformat(raw.replace('Z','+00:00')).timestamp()
for c in json.load(open('/tmp/cache-janitor-list.json')):
    if not c['key'].startswith('sccache/'): continue
    if c['ref'] in closed: continue
    if ts(c) < cutoff: print(f\"{c['id']}\t{c['sizeInBytes']}\t{c['key']}\")
")

# ---------- 规则 C:codeql-overlay 每语言留最新 1 份 ----------
echo "== 规则 C:codeql-overlay 去重 =="
while IFS=$'\t' read -r id size key; do
  delete_cache "$id" "$key" "$size" "C_codeql"
done < <(python3 -c "
import json, os
closed = set(os.environ.get('CLOSED_REFS','').split())
entries = [c for c in json.load(open('/tmp/cache-janitor-list.json')) if c['key'].startswith('codeql-overlay-base-database-') and c['ref'] not in closed]
groups = {}
for c in entries:
    # key 形如 codeql-overlay-base-database-1-<sha>-<lang>-<version>-...
    # 按 (语言, 作用域) 分组:缓存作用域互不可见,跨 ref 只留最新会把 main 的
    # 缓存换成 PR 作用域的(main 读不到),等于删掉 main 的可用缓存。
    parts = c['key'].split('-')
    lang = parts[6] if len(parts) > 6 else c['key']
    groups.setdefault((lang, c['ref']), []).append(c)
for group, items in groups.items():
    items.sort(key=lambda c: c['createdAt'], reverse=True)
    for c in items[1:]: print(f\"{c['id']}\t{c['sizeInBytes']}\t{c['key']}\")
")

# ---------- 规则 D:node-cache 每平台留最新 1 份 ----------
echo "== 规则 D:node-cache 去重 =="
while IFS=$'\t' read -r id size key; do
  delete_cache "$id" "$key" "$size" "D_node"
done < <(python3 -c "
import json, os
closed = set(os.environ.get('CLOSED_REFS','').split())
entries = [c for c in json.load(open('/tmp/cache-janitor-list.json')) if c['key'].startswith('node-cache-') and c['ref'] not in closed]
groups = {}
for c in entries:
    # key 形如 node-cache-<platform>-npm-<hash>,平台段取 npm- 之前的部分
    # 按 (平台, 作用域) 分组:同规则 C,跨 ref 只留最新会误删 main 的可用缓存。
    platform = c['key'].split('-npm-')[0]
    groups.setdefault((platform, c['ref']), []).append(c)
for group, items in groups.items():
    items.sort(key=lambda c: c['createdAt'], reverse=True)
    for c in items[1:]: print(f\"{c['id']}\t{c['sizeInBytes']}\t{c['key']}\")
")

echo "== 汇总 =="
echo "A_closed_pr=$STAT_A_CLOSED_PR B_sccache_stale=$STAT_B_SCCACHE_STALE C_codeql=$STAT_C_CODEQL D_node=$STAT_D_NODE"
echo "释放空间约 $(( DELETED_BYTES / 1048576 ))MB(dry-run=$DRY_RUN)"
