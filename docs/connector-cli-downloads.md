# 连接器 CLI 下载源与加速

连接器 CLI（钉钉 dws、飞书 lark-cli、企微 wecom-cli）首次使用时按平台 lock 表
（`pinvou3-app/src-tauri/resources/platforms/<os>/<arch>/bundle/connectors/connectors.lock.json`）
安装固定版本。每个候选下载源的字节都要过 lock 内的 `archiveSha256` /
`binarySha256` 校验，不匹配即自动落到下一候选源——镜像只影响下载速度，
不影响安装内容的完整性。

下载候选按序尝试：

1. `PINVOU3_GITHUB_ASSET_MIRROR_PREFIX` 派生的加速地址（仅当官方源在
   `github.com` 时生效）；
2. lock 表内审核过的国内镜像（当前仅 wecom-cli 配置 `registry.npmmirror.com`，
   与官方 npm 包同路径同步）；
3. 官方源兜底。

## GitHub 加速前缀（可选环境变量）

dws 与 lark-cli 只发布为 GitHub Release 资产，暂无厂商国内镜像。github.com
访问受限的网络环境可设置 `PINVOU3_GITHUB_ASSET_MIRROR_PREFIX` 指向 gh-proxy
风格的加速服务（拼接规则 `<prefix>https://github.com/...`，前缀带不带结尾
斜杠均可）：

```bash
PINVOU3_GITHUB_ASSET_MIRROR_PREFIX=https://your-gh-proxy.example pinvou3
```

前缀仅作用于 `github.com` 地址，不会包进其他站点；加速地址必须在 HTTPS
服务上。拼错或不可达的加速地址只会被下载前的 HTTPS 复查或下载后的
SHA-256 校验拦下并落到下一候选，不会安装未过校验的字节。

## 腾讯会议 CLI（tmeet）

tmeet（`@tencentcloud/tmeet`，腾讯会议连接器使用）来自 npm，按固定版本安装：
默认 registry 整体失败后，对该次调用追加
`--registry=https://registry.npmmirror.com` 重试一次。两次尝试的输出都保留在
`~/.pinvou3/cli-install.log`（追加写，超过 8 MiB 自动轮转为 `.old`），阶段边界
有标记行可区分。与上面的归档下载不同，npm 安装没有应用侧制品校验，完整性依赖
TLS 与镜像源的同步保真；registry 标志仅作用于单条命令，Pinvou 不会写入或修改
用户的 npm 配置（npm 自身仍会照常读取其 `.npmrc` 的 prefix/cache/auth 等配置）。
