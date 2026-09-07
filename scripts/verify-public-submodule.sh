#!/usr/bin/env bash
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PINVOU_CODEWHALE_PATH="CodeWhale"
PINVOU_CODEWHALE_URL="https://github.com/Pinvou/CodeWhale.git"
PINVOU_CODEWHALE_BRANCH="pinvou3-clean"
PINVOU_CODEWHALE_TAG="pinvou-v0.9.12-r1"
MAX_ATTEMPTS=3

if [[ $# -ne 0 ]]; then
  echo "unknown argument: $1" >&2
  exit 2
fi

actual_path="$(git -C "$REPO" config -f .gitmodules --get submodule.CodeWhale.path)"
actual_url="$(git -C "$REPO" config -f .gitmodules --get submodule.CodeWhale.url)"

if [[ "$actual_path" != "$PINVOU_CODEWHALE_PATH" ]]; then
  echo "错误：CodeWhale submodule path 应为 $PINVOU_CODEWHALE_PATH，实际为 $actual_path" >&2
  exit 1
fi

if [[ "$actual_url" != "$PINVOU_CODEWHALE_URL" ]]; then
  echo "错误：CodeWhale submodule 必须使用公开 URL $PINVOU_CODEWHALE_URL" >&2
  exit 1
fi

if git -C "$REPO" config -f .gitmodules --get submodule.CodeWhale.branch >/dev/null; then
  echo "错误：.gitmodules 不得配置浮动的 submodule.CodeWhale.branch" >&2
  exit 1
fi

gitlink="$(
  git -C "$REPO" ls-files --stage -- "$PINVOU_CODEWHALE_PATH" |
    awk '$1 == "160000" { print $2 }'
)"

if [[ ! "$gitlink" =~ ^[0-9a-f]{40}$ ]]; then
  echo "错误：无法从索引读取唯一的 CodeWhale gitlink commit" >&2
  exit 1
fi

remote_refs=""
for attempt in $(seq 1 "$MAX_ATTEMPTS"); do
  if remote_refs="$(git ls-remote --heads --tags "$PINVOU_CODEWHALE_URL" 2>/dev/null)"; then
    break
  fi
  if [[ "$attempt" -eq "$MAX_ATTEMPTS" ]]; then
    echo "错误：${MAX_ATTEMPTS} 次尝试后仍无法读取公开 CodeWhale refs" >&2
    exit 1
  fi
  sleep "$attempt"
done

branch_target="$(
  printf '%s\n' "$remote_refs" |
    awk -v ref="refs/heads/${PINVOU_CODEWHALE_BRANCH}" '$2 == ref { print $1 }'
)"
tag_direct="$(
  printf '%s\n' "$remote_refs" |
    awk -v ref="refs/tags/${PINVOU_CODEWHALE_TAG}" '$2 == ref { print $1 }'
)"
tag_peeled="$(
  printf '%s\n' "$remote_refs" |
    awk -v ref="refs/tags/${PINVOU_CODEWHALE_TAG}^{}" '$2 == ref { print $1 }'
)"
tag_target="${tag_peeled:-$tag_direct}"

if [[ "$branch_target" != "$gitlink" ]]; then
  echo "错误：${PINVOU_CODEWHALE_BRANCH} 为 ${branch_target:-<不存在>}，父仓 gitlink 为 $gitlink" >&2
  exit 1
fi

if [[ "$tag_target" != "$gitlink" ]]; then
  echo "错误：${PINVOU_CODEWHALE_TAG} 解引用为 ${tag_target:-<不存在>}，父仓 gitlink 为 $gitlink" >&2
  exit 1
fi

if [[ "$branch_target" != "$tag_target" ]]; then
  echo "错误：公开维护分支与不可变标签未指向同一 commit" >&2
  exit 1
fi

echo "公开 CodeWhale 基线校验通过：${PINVOU_CODEWHALE_BRANCH} = ${PINVOU_CODEWHALE_TAG} = $gitlink"
