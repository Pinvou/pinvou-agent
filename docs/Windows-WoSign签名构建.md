# Windows WoSign 签名构建

Windows NSIS 发布构建通过项目内
`pinvou3-app/src-tauri/resources/windows/wosign/wosigncodecmd.exe`
完成签名，不依赖服务器上的外部工具目录。该工具仅供构建阶段使用，
没有配置为 Tauri bundle 资源，不会随应用安装包发布。

## 签名参数

代码签名证书 SHA1 指纹和 UKey 密码当前固定配置在
`pinvou3-app/src-tauri/resources/windows/wosign/sign.ps1` 中，构建前不需要设置对应环境变量。

可选环境变量：

- `PINVOU3_WOSIGN_TIMESTAMP_URL`：RFC 3161 时间戳地址，默认使用 `http://timestamp.digicert.com`。
- `PINVOU3_WOSIGN_TOOL_PATH`：`wosigncodecmd.exe` 的可选覆盖路径；默认使用项目签名脚本同目录下的工具。

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

两个命令都会先执行 `check:wosign`，签名参数无效或项目签名工具缺失时立即终止。Tauri 在写入 NSIS bundle 类型后调用同一签名脚本，依次签署主程序、NSIS 卸载程序和最终安装器。
