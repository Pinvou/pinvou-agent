# Quickstart：Windows 内置 7z

## 前置检查

1. 确认当前 feature：

   ```powershell
   Get-Content .specify\feature.json
   ```

   应指向：

   ```json
   { "feature_directory": "specs/017-bundle-windows-7z" }
   ```

2. 确认源目录存在：

   ```powershell
   Test-Path "C:\Program Files\7-Zip"
   Get-ChildItem "C:\Program Files\7-Zip"
   ```

3. 验证源包格式能力：

   ```powershell
   & "C:\Program Files\7-Zip\7z.exe" i
   ```

   当前已验证：输出包含 `zip`、`7z`、`Rar`、`Rar5`，并列出 `Rar1/2/3/5` 解码器。

4. 验证裁剪后的最小运行文件集：

   ```powershell
   $tmp = Join-Path $env:TEMP "pinvou3-7z-minimal-test"
   Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
   New-Item -ItemType Directory -Path $tmp | Out-Null
   Copy-Item "C:\Program Files\7-Zip\7z.exe" $tmp
   Copy-Item "C:\Program Files\7-Zip\7z.dll" $tmp
   & (Join-Path $tmp "7z.exe") i
   Remove-Item -LiteralPath $tmp -Recurse -Force
   ```

   输出应仍包含 `Rar`、`Rar5` 和 `Rar1/2/3/5` 解码器。

## 实现后验证

### 本次实现验证记录

- Windows 资源目录实际随附：`7z.exe`、`7z.dll`、`License.txt`、`readme.txt`、`README.md`。
- 已构建 MSI：`pinvou3-app/src-tauri/target/release/bundle/msi/pinvou3_0.4.11_x64_en-US.msi`。
- 已通过 `msiexec /a` 管理解包验证 MSI 中 `7zip` 目录仅包含裁剪后的 6 个文件，不包含 Shell 插件、GUI、SFX、CHM、卸载器或 `Lang` 目录。
- 已验证裁剪后的 `7z.exe` + `7z.dll` 可执行 `7z.exe i` 并显示 `Rar`、`Rar5` 和 `Rar1/2/3/5` 解码器。
- 已执行 Rust 编译检查和聚焦 archive/依赖体检测试；当前 Windows 环境仍有系统级 7z，未执行“无系统级 7z 的手工上传样本验证”。

1. 静态检查：

   ```powershell
   cargo check --manifest-path pinvou3-app\src-tauri\Cargo.toml
   ```

2. 聚焦测试：

   ```powershell
   cargo test --manifest-path pinvou3-app\src-tauri\Cargo.toml archive --lib
   ```

3. Windows 无系统级 7z 验证：

   ```powershell
   where.exe 7z
   ```

   在系统级命令不可用时，启动应用上传 zip/7z 样本，应能解析成功。

4. MSI 安装布局验证：

   ```powershell
   Test-Path "C:\Program Files\pinvou3\7zip"
   Get-ChildItem "C:\Program Files\pinvou3\7zip"
   ```

   安装目录中应包含 `7z.exe`、`7z.dll`、`License.txt`、`readme.txt`、`README.md`，不应包含 `7-zip.dll`、`7-zip32.dll`、`7zFM.exe`、`7zG.exe`、`History.txt`、SFX、CHM、卸载器或 `Lang` 目录。

5. 依赖体检验证：

   - Windows：压缩包项不提示 `p7zip-full`。
   - Linux：缺少 7z 时仍提示 `p7zip-full`。

6. RAR 验证：

   - 上传 rar 样本应解析成功。
   - 若失败，优先检查 MSI 是否完整携带 `7z.exe` 和 `7z.dll`。

## 手动样本建议

- `archive-basic.zip`：包含一个中文 `.txt`
- `archive-basic.7z`：包含一个中文 `.txt`
- `archive-basic.rar`：包含一个中文 `.txt`
- `archive-empty.zip`：空压缩包
- `archive-corrupt.zip`：损坏压缩包
- `archive-nested.zip`：内部包含另一个 `.zip`

## 回归关注

- 不要改变 PDF、Office、OCR、邮件解析路径。
- 不要改 DeepSeek-TUI submodule。
- 不要对 `file_ingest.rs` 做整文件格式化。
- 不要让 Linux 因 Windows 内置资源跳过 `p7zip-full` 检查。
