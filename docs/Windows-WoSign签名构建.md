# Windows WoSign 签名构建

Windows NSIS 发布构建通过项目内的 `wosigncodecmd.exe` 完成签名，
同时要求同目录存在配套的 `wosigncode.exe`，不依赖服务器上的外部工具目录。
两个工具仅供构建阶段使用，没有配置为 Tauri bundle 资源，不会随应用安装包发布。

## 签名参数

代码签名证书 SHA1 指纹和 UKey 密码当前固定配置在
`pinvou3-app/src-tauri/resources/windows/wosign/sign.ps1` 中，构建前不需要设置对应环境变量。

可选环境变量：

- `PINVOU3_WOSIGN_TIMESTAMP_URL`：RFC 3161 时间戳地址，默认使用 `http://timestamp.digicert.com`。

签名工具路径不可通过环境变量覆盖，构建始终使用项目内与 `sign.ps1` 同目录的
`wosigncodecmd.exe` 和 `wosigncode.exe`，并以该目录作为沃通命令的工作目录。
签名命令保留 `/isf` 以跳过已有签名，并继续通过 `/tr` 请求 RFC3161 时间戳。
当前以沃通命令返回码判断执行结果，不再额外校验 Authenticode 状态、签名证书指纹和时间戳证书。
Tauri 的自定义签名命令从 `src-tauri` 目录执行，因此配置中的脚本路径必须写为
`resources/windows/wosign/sign.ps1`，不得再次添加 `src-tauri/` 前缀。

证书或 UKey 密码变化后，需要同步修改项目内签名脚本。

## 构建命令

直接编译并生成 NSIS 安装器：

```powershell
npm run build:nsis
```

使用已完成的 NSIS staging 生成安装器：

```powershell
npm run bundle:nsis
```

两个命令都会先执行 `check:wosign`，签名参数无效、命令行工具或配套工具缺失时立即终止。Tauri 在写入 NSIS bundle 类型后调用同一签名脚本，依次签署主程序、NSIS 卸载程序和最终安装器。构建已启用 Tauri verbose 日志；若签名失败，Jenkins 日志会显示签名阶段、目标文件、沃通输出和退出码。
