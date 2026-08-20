#!/usr/bin/env bash
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PINVOU_CODEWHALE_PATH="CodeWhale"
PINVOU_CODEWHALE_URL="https://github.com/Pinvou/CodeWhale.git"
PINVOU_CODEWHALE_PUBLISHED_TAG="pinvou-v0.9.5-r7"
PINVOU_CODEWHALE_PUBLISHED_HEAD="a36e6cd533024cfe5724bae21875aea42b2ed87a"
PINVOU_CODEWHALE_FEATURE_REF="refs/heads/feat/pinvouos-front-round-policy"

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

remote_refs="$(git ls-remote "$PINVOU_CODEWHALE_URL")"
resolve_remote_tag() {
  local tag="$1" peeled direct
  peeled="$(
    printf '%s\n' "$remote_refs" |
      awk -v ref="refs/tags/${tag}^{}" '$2 == ref { print $1; exit }'
  )"
  if [[ -n "$peeled" ]]; then
    printf '%s\n' "$peeled"
    return 0
  fi
  direct="$(
    printf '%s\n' "$remote_refs" |
      awk -v ref="refs/tags/${tag}" '$2 == ref { print $1; exit }'
  )"
  printf '%s\n' "$direct"
}

tag_target="$(resolve_remote_tag "$PINVOU_CODEWHALE_PUBLISHED_TAG")"

if [[ "$tag_target" != "$PINVOU_CODEWHALE_PUBLISHED_HEAD" ]]; then
  echo "错误：${PINVOU_CODEWHALE_PUBLISHED_TAG} 应固定公开基线 $PINVOU_CODEWHALE_PUBLISHED_HEAD，实际为 ${tag_target:-<不存在>}" >&2
  exit 1
fi

feature_target="$(
  printf '%s\n' "$remote_refs" |
    awk -v ref="$PINVOU_CODEWHALE_FEATURE_REF" '$2 == ref { print $1 }'
)"

if [[ "$feature_target" != "$gitlink" ]]; then
  echo "错误：公开 feature ref ${PINVOU_CODEWHALE_FEATURE_REF} 指向 ${feature_target:-<不存在>}，父仓 gitlink 为 $gitlink" >&2
  exit 1
fi

if ! printf '%s\n' "$remote_refs" | awk -v sha="$gitlink" '$1 == sha { found = 1 } END { exit !found }'; then
  echo "错误：父仓 gitlink $gitlink 无法从公开 CodeWhale refs 到达" >&2
  exit 1
fi

echo "公开 CodeWhale 校验通过：${PINVOU_CODEWHALE_PUBLISHED_TAG} -> $tag_target；${PINVOU_CODEWHALE_FEATURE_REF} -> $gitlink"
