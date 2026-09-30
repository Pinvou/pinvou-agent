# Connector CLI download sources and acceleration

Connector CLIs (DingTalk `dws`, Feishu/Lark `lark-cli`, WeCom `wecom-cli`) install
a pinned version on first use according to the per-platform lock file
(`pinvou3-app/src-tauri/resources/platforms/<os>/<arch>/bundle/connectors/connectors.lock.json`).
Bytes from every candidate download source are verified against the lock's
`archiveSha256` / `binarySha256`; a mismatch automatically falls through to the
next candidate — mirrors only affect download speed, never the integrity of what
gets installed.

Candidates are tried in order:

1. An accelerated URL derived from `PINVOU3_GITHUB_ASSET_MIRROR_PREFIX` (only
   when the official source lives on `github.com`);
2. A reviewed China mirror recorded in the lock file (currently only wecom-cli
   configures `registry.npmmirror.com`, which mirrors the official npm package
   at the same path);
3. The official source as the final fallback.

## GitHub asset mirror prefix (optional environment variable)

dws and lark-cli are only published as GitHub Release assets and have no vendor
China mirror. On networks where `github.com` is restricted, set
`PINVOU3_GITHUB_ASSET_MIRROR_PREFIX` to a gh-proxy-style acceleration service
(joined as `<prefix>https://github.com/...`; the prefix may omit the trailing
slash):

```bash
PINVOU3_GITHUB_ASSET_MIRROR_PREFIX=https://your-gh-proxy.example pinvou3
```

The prefix applies only to `github.com` URLs; other sites are never wrapped. The
resulting URL must be served over HTTPS. A mistyped or unreachable prefix is
caught by the pre-download HTTPS check or the post-download SHA-256 verification
and falls through to the next candidate — unverified bytes are never installed.

## Tencent Meeting CLI (tmeet)

tmeet (`@tencentcloud/tmeet`, used by the Tencent Meeting connector) comes from
npm and installs a pinned version: when the default registry attempt fails
outright, the same invocation is retried once with
`--registry=https://registry.npmmirror.com`. Output from both attempts is kept
in `~/.pinvou3/cli-install.log` (append-only, rotated to `.old` past 8 MiB) with
marker lines separating the stages. Unlike the archive downloads above, npm
installs have no app-side artifact verification; integrity rests on TLS plus the
mirror's registry-sync fidelity. The registry flag applies to that single
command only — Pinvou never writes or modifies the user's npm configuration
(npm itself still reads its usual `.npmrc` settings such as prefix/cache/auth).
