# Quickstart：Windows 内置 Poppler 安装验证

## 前置条件

- 当前分支：`010-bundle-windows-poppler`
- Poppler 源目录存在：`C:\Users\z27014\Downloads\poppler-26.02.0`
- 源目录根部包含 `pdftotext.exe` 和 `pdftoppm.exe`
- Windows 构建环境可运行 Tauri MSI 打包

## 开发验证

1. 确认 Poppler 源目录结构：

   ```powershell
   Get-ChildItem C:\Users\z27014\Downloads\poppler-26.02.0\pdftotext.exe
   Get-ChildItem C:\Users\z27014\Downloads\poppler-26.02.0\pdftoppm.exe
   ```

2. 确认仓库资源目录包含 Poppler：

   ```powershell
   Get-ChildItem pinvou3-app\src-tauri\resources\windows\poppler\pdftotext.exe
   Get-ChildItem pinvou3-app\src-tauri\resources\windows\poppler\pdftoppm.exe
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

2. 在未安装 Poppler、PATH 中不包含 Poppler 的 Windows 环境安装 MSI。

3. 检查安装目录：

   ```powershell
   Get-ChildItem "$env:ProgramFiles\pinvou3\poppler\pdftotext.exe"
   Get-ChildItem "$env:ProgramFiles\pinvou3\poppler\pdftoppm.exe"
   ```

   如果安装目录不同，以实际安装目录为准。

4. 检查 PATH 环境项：

   ```powershell
   [Environment]::GetEnvironmentVariable("PATH", "Machine") -split ";" |
     Where-Object { $_ -like "*pinvou3*poppler*" }
   ```

5. 启动 pinvou，上传文字层 PDF，确认附件解析成功。

6. 打开依赖体检，确认不再出现 Poppler/PDF 文本提取缺失提示。

7. 删除或重命名安装目录中的 `poppler\pdftotext.exe`，再次上传 PDF，确认错误提示指向安装内容异常或修复安装。

## 回归边界

- Linux 依赖体检仍应显示 Poppler 相关系统依赖。
- Windows 上其他依赖体检项仍应正常展示。
- 安装目录包含空格或中文字符时，PDF 上传仍应成功。

## 实际执行记录（2026-06-23）

- 已确认源目录 `C:\Users\z27014\Downloads\poppler-26.02.0` 存在，源目录根部包含 39 个文件，包含 `pdftotext.exe`、`pdftoppm.exe` 和运行所需 DLL。
- 已复制到 `pinvou3-app/src-tauri/resources/windows/poppler/`，并补充 `README.md` 记录来源、版本和完整性快照。
- MSI 资源配置：`tauri.conf.json` 启用 `deb` + `msi` targets，并将 `resources/windows/poppler/` 映射到安装资源目标 `poppler`。
- MSI PATH 配置：`resources/windows/poppler-path.wxs` 定义 `PopplerPathEnvironment`，`tauri.conf.json` 通过 `componentRefs` 引用该 component。MSI `Environment` 表验证结果包含：

  ```text
  PinvouPopplerPath|=-*PATH|[~];[INSTALLDIR]poppler|PopplerPathEnvironment
  ```

- 已执行 Rust 验证：

  ```powershell
  cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml file_ingest --lib
  # 11 passed; 6 ignored

  cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml windows_path --lib
  # 3 passed

  cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml
  # passed; only existing warnings
  ```

- 已执行 MSI 构建：

  ```powershell
  npm run build -- --bundles msi
  # produced pinvou3-app/src-tauri/target/release/bundle/msi/pinvou3_0.4.7_x64_en-US.msi
  ```

- 已执行 MSI 管理解包验证（不安装系统）：

  ```powershell
  msiexec /a pinvou3_0.4.7_x64_en-US.msi /qn TARGETDIR=$env:TEMP\pinvou3-msi-poppler-check
  Get-ChildItem "$env:TEMP\pinvou3-msi-poppler-check\PFiles\pinvou3\poppler\pdftotext.exe"
  Get-ChildItem "$env:TEMP\pinvou3-msi-poppler-check\PFiles\pinvou3\poppler\pdftoppm.exe"
  # both files present
  ```

- 未执行完整安装后的 UI smoke：未在当前回合安装 MSI 到系统并手动上传 PDF。已用 MSI 构建、MSI 表查询、管理解包、Rust 单测和 cargo check 覆盖主要风险。
