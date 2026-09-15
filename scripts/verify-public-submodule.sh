#!/usr/bin/env bash
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PINVOU_CODEWHALE_PATH="CodeWhale"
PINVOU_CODEWHALE_URL="https://github.com/Pinvou/CodeWhale.git"
PINVOU_CODEWHALE_BRANCH="pinvou3-clean"
PINVOU_CODEWHALE_TAG="pinvou-v0.9.12-r1"
# 过渡期豁免（fork-policy.md 第 0 节）：两次 rN 收口之间，父仓 gitlink 可沿受保护
# 维护分支领先不可变 tag 前进。TRANSITION_BASELINE 非空即启用过渡期断言：
# tag 必须钉在其收口 commit，gitlink 必须等于公开维护分支头（上方已断言）且领先 tag。
# rN 收口时：把 TAG 更新为新 tag、删除 TRANSITION_BASELINE，恢复三方相等断言。
TRANSITION_BASELINE="1fafee7e26b60a59457a43bce50c63aa2ad9dbaf"
MAX_ATTEMPTS=3

if [[ $# -ne 0 ]]; then
  echo "unknown argument: $1" >&2
  exit 2
fi

actual_path="$(git -C "$REPO" config -f .gitmodules --get submodule.CodeWhale.path)"
actual_url="$(git -C "$REPO" config -f .gitmodules --get submodule.CodeWhale.url)"

if [[ "$actual_path" != "$PINVOU_CODEWHALE_PATH" ]]; then
  echo "错误：CodeWhale submodule path 应为 ${PINVOU_CODEWHALE_PATH}，实际为 $actual_path" >&2
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
  if remote_refs="$(
    git ls-remote "$PINVOU_CODEWHALE_URL" \
      "refs/heads/${PINVOU_CODEWHALE_BRANCH}" \
      "refs/tags/${PINVOU_CODEWHALE_TAG}" \
      "refs/tags/${PINVOU_CODEWHALE_TAG}^{}" \
      2>/dev/null
  )"; then
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

if [[ -n "$TRANSITION_BASELINE" ]]; then
  if [[ "$tag_target" != "$TRANSITION_BASELINE" ]]; then
    echo "错误：${PINVOU_CODEWHALE_TAG} 解引用为 ${tag_target:-<不存在>}，过渡期内应钉在收口 ${TRANSITION_BASELINE}；若这是新切的 rN tag，请把 TAG 常量更新为新 tag 名并清除 TRANSITION_BASELINE，不得移动已发布的不可变 tag" >&2
    exit 1
  fi
  if [[ "$tag_target" == "$gitlink" ]]; then
    echo "错误：gitlink 已追平 ${PINVOU_CODEWHALE_TAG} 收口，请切除新 tag 并清除 TRANSITION_BASELINE" >&2
    exit 1
  fi
  echo "公开 CodeWhale 基线过渡期校验通过：${PINVOU_CODEWHALE_BRANCH} = gitlink = ${gitlink}；${PINVOU_CODEWHALE_TAG} 钉在收口 ${tag_target}"
  exit 0
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
