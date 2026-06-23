# Quickstart：Windows 内置 Pandoc 安装验证

## 前置条件

- 当前分支：`013-bundle-windows-pandoc`
- Pandoc 源目录存在：`C:\Users\z27014\Downloads\pandoc-3.10`
- 源目录根部包含 `pandoc.exe`
- Windows 构建环境可运行 Tauri MSI 打包

## 开发验证

1. 确认 Pandoc 源目录结构：

   ```powershell
   Get-ChildItem C:\Users\z27014\Downloads\pandoc-3.10\pandoc.exe
   Get-ChildItem C:\Users\z27014\Downloads\pandoc-3.10\COPYRIGHT.txt
   ```

2. 确认仓库资源目录包含 Pandoc：

   ```powershell
   Get-ChildItem pinvou3-app\src-tauri\resources\windows\pandoc\pandoc.exe
   Get-ChildItem pinvou3-app\src-tauri\resources\windows\pandoc\COPYRIGHT.txt
   ```

3. 运行 Rust 验证：

   ```powershell
   cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml file_ingest --lib
   cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml windows_path --lib
   cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml
   ```

## MSI 验收

1. 构建 Windows MSI：

   ```powershell
   cd pinvou3-app
   npm run build -- --bundles msi
   ```

2. 在未安装 Pandoc、PATH 中不包含 Pandoc 的 Windows 环境安装 MSI。

3. 检查安装目录：

   ```powershell
   Get-ChildItem "$env:ProgramFiles\pinvou3\pandoc\pandoc.exe"
   ```

   如果安装目录不同，以实际安装目录为准。

4. 检查 PATH 环境项：

   ```powershell
   [Environment]::GetEnvironmentVariable("PATH", "Machine") -split ";" |
     Where-Object { $_ -like "*pinvou3*pandoc*" }
   ```

5. 启动 pinvou，上传依赖 Pandoc 解析的支持文档，确认附件解析成功。

6. 打开依赖体检，确认不再出现 Pandoc/现代文档解析缺失提示。

7. 删除或重命名安装目录中的 `pandoc\pandoc.exe`，再次上传相关文档，确认错误提示指向安装内容异常或修复安装。

## 回归边界

- Linux 依赖体检仍应显示 Pandoc 相关系统依赖。
- Windows 上其他依赖体检项仍应正常展示。
- 安装目录包含空格或中文字符时，文档上传仍应成功。

## 规划期源目录快照（2026-06-23）

源目录 `C:\Users\z27014\Downloads\pandoc-3.10` 已确认存在，根部文件包括：

```text
COPYING.rtf
COPYRIGHT.txt
MANUAL.html
pandoc.exe
```

## 实际执行记录（2026-06-23）

- 已确认源目录 `C:\Users\z27014\Downloads\pandoc-3.10` 存在，源目录根部包含 4 个文件：

  ```text
  COPYING.rtf
  COPYRIGHT.txt
  MANUAL.html
  pandoc.exe
  ```

- 已复制到 `pinvou3-app/src-tauri/resources/windows/pandoc/`，并补充 `README.md` 记录来源、版本和完整性快照。
- 已确认资源内 Pandoc 版本：

  ```powershell
  .\pinvou3-app\src-tauri\resources\windows\pandoc\pandoc.exe --version
  # pandoc 3.10
  ```

- MSI 资源配置：`tauri.conf.json` 将 `resources/windows/pandoc/` 映射到安装资源目标 `pandoc`，并保留既有 `poppler` 资源映射。
- MSI PATH 配置：`resources/windows/pandoc-path.wxs` 定义 `PandocPathEnvironment`，`tauri.conf.json` 通过 `componentRefs` 引用该 component。MSI `Environment` 表验证结果包含：

  ```text
  PinvouPopplerPath|=-*PATH|[~];[INSTALLDIR]poppler|PopplerPathEnvironment
  PinvouPandocPath|=-*PATH|[~];[INSTALLDIR]pandoc|PandocPathEnvironment
  ```

- 已执行 Rust 验证：

  ```powershell
  cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml pandoc_tool_path_returns_non_empty_program --lib
  # 1 passed

  cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml windows_path --lib
  # 6 passed

  cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml file_ingest --lib
  # 14 passed; 6 ignored

  cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml
  # passed; only existing warnings
  ```

- 已执行 MSI 构建：

  ```powershell
  cd pinvou3-app
  npm run build -- --bundles msi
  # produced pinvou3-app/src-tauri/target/release/bundle/msi/pinvou3_0.4.7_x64_en-US.msi
  ```

- 已执行 MSI 管理解包验证（不安装系统）：

  ```powershell
  msiexec /a pinvou3_0.4.7_x64_en-US.msi /qn TARGETDIR=$env:TEMP\pinvou3-msi-pandoc-check
  # PFiles\pinvou3\pandoc\pandoc.exe present
  # PFiles\pinvou3\pandoc\COPYING.rtf present
  # PFiles\pinvou3\pandoc\COPYRIGHT.txt present
  # PFiles\pinvou3\pandoc\MANUAL.html present
  ```

- 已执行 release 目录旁内置 Pandoc 解析 smoke：

  ```powershell
  pinvou3-app\src-tauri\target\release\pandoc\pandoc.exe sample.md -o sample.docx
  pinvou3-app\src-tauri\target\release\pandoc\pandoc.exe -t markdown sample.docx
  # 输出包含 "Windows bundled Pandoc works."
  ```

- 未执行完整安装后的 UI 手动上传 smoke：当前回合未静默安装 MSI 到系统并通过 UI 手动上传 `docx`。已用资源版本检查、Rust 单测、cargo check、MSI 构建、MSI 表查询、管理解包和 release 内置 Pandoc 转换 smoke 覆盖主要风险；完整 UI smoke 可在安装新版 MSI 后补验。
