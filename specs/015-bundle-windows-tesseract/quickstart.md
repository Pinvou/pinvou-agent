# 快速开始：Windows 内置 Tesseract OCR 验收

## 前置条件

- 当前分支为 `015-bundle-windows-tesseract`。
- 已准备受控 Windows Tesseract runtime，包含 `tesseract.exe`、运行所需 DLL、`tessdata/chi_sim.traineddata`、`tessdata/eng.traineddata` 和许可证/来源说明。
- Windows 测试机未安装系统级 Tesseract，或已确认系统 PATH 中的 Tesseract 不会被优先使用。
- 已有中英文扫描件 PDF 样本，页数不超过 3 页；如果没有现成样本，可用任意包含中英文文字的文档打印为 PDF 后再扫描或截图转为无文字层 PDF。

## 构建前检查

```powershell
Test-Path pinvou3-app/src-tauri/resources/windows/tesseract/tesseract.exe
Test-Path pinvou3-app/src-tauri/resources/windows/tesseract/tessdata/chi_sim.traineddata
Test-Path pinvou3-app/src-tauri/resources/windows/tesseract/tessdata/eng.traineddata
```

确认 `pinvou3-app/src-tauri/resources/windows/tesseract/` 中存在许可证或来源说明文件。

## 自动检查

```powershell
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml windows_path --lib
cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml file_ingest --lib
cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml
```

## MSI 构建与安装检查

```powershell
Set-Location pinvou3-app
npm run tauri build
```

可选解包检查 MSI 内容：

```powershell
$msi = "pinvou3-app/src-tauri/target/release/bundle/msi/pinvou3_0.4.11_x64_en-US.msi"
$out = "$env:TEMP\pinvou3-msi-check"
Remove-Item -Recurse -Force $out -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $out | Out-Null
msiexec /a $msi /qn TARGETDIR=$out
Test-Path "$out\pinvou3\tesseract\tesseract.exe"
Test-Path "$out\pinvou3\tesseract\tessdata\chi_sim.traineddata"
Test-Path "$out\pinvou3\tesseract\tessdata\eng.traineddata"
Test-Path "$out\pinvou3\tesseract\LICENSE"
```

安装生成的 MSI 后检查：

```powershell
Test-Path "$env:ProgramFiles\pinvou3\tesseract\tesseract.exe"
Test-Path "$env:ProgramFiles\pinvou3\tesseract\tessdata\chi_sim.traineddata"
Test-Path "$env:ProgramFiles\pinvou3\tesseract\tessdata\eng.traineddata"
```

实际安装目录以 MSI 安装结果为准；如果安装到用户目录或自定义目录，应检查对应 `{安装目录}/tesseract`。

## 功能验收

1. 在未安装系统级 Tesseract 的 Windows 测试机安装 pinvou。
2. 打开应用并上传中英文扫描件 PDF。
3. 验证 PDF 首先尝试文字层解析，文字层为空时进入 OCR 兜底。
4. 验证 3 页以内样本在 30 秒内返回可读 OCR 文本。
5. 验证结果提示 OCR 内容可能存在识别误差。
6. 打开依赖体检页，确认没有要求用户手动安装 Tesseract 的阻断提示。

## 破坏性验收

1. 关闭 pinvou。
2. 临时移走 `{安装目录}/tesseract/tesseract.exe`。
3. 重新打开 pinvou 并上传扫描件 PDF。
4. 验证错误提示指向修复安装或重新安装 pinvou，而不是手动安装第三方 Tesseract。
5. 恢复 `tesseract.exe`，再临时移走 `tessdata/chi_sim.traineddata`。
6. 上传中文扫描件 PDF，验证提示能定位到 OCR 语言数据缺失或安装内容异常。
7. 恢复文件后重新上传，确认 OCR 恢复正常。

记录格式：

```text
测试环境：
MSI 路径：
安装目录：
是否预装系统级 Tesseract：
样本 PDF：

正常 OCR：
- 结果：
- 耗时：
- 备注：

删除 tesseract.exe：
- 操作：
- 用户提示：
- 是否指向修复安装：

删除 tessdata/chi_sim.traineddata：
- 操作：
- 用户提示：
- 是否指向语言数据缺失或安装内容异常：
```

## 回归范围

- 有文字层 PDF 仍应优先返回 `pdftotext` 文本。
- 普通图片上传不应因为本 feature 变成 OCR 主路径。
- Linux 依赖体检仍应提示 `tesseract-ocr tesseract-ocr-chi-sim poppler-utils`。

## 当前执行记录

- 资源目录 smoke：`pinvou3-app/src-tauri/resources/windows/tesseract/tesseract.exe --version` 成功，版本为 `v5.5.0.20241111`。
- 语言数据 smoke：`--list-langs --tessdata-dir pinvou3-app/src-tauri/resources/windows/tesseract/tessdata` 成功，仅列出 `chi_sim`、`eng`。
- 资源体积：`pinvou3-app/src-tauri/resources/windows/tesseract/` 当前约 78.4 MB，包含 62 个文件。
- Rust 路径测试：`cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml windows_path --lib` 通过，8 passed。
- 附件解析测试：`cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml file_ingest --lib` 通过，16 passed，6 ignored。
- Rust 编译检查：`cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml` 通过，仅有既有 warning。
- MSI 构建：`npm run tauri build` 通过，产物为 `pinvou3-app/src-tauri/target/release/bundle/msi/pinvou3_0.4.11_x64_en-US.msi`。
- MSI 解包检查：`msiexec /a` 返回 0，解包目录包含 `PFiles\pinvou3\tesseract\tesseract.exe`、`PFiles\pinvou3\tesseract\tessdata\chi_sim.traineddata`、`PFiles\pinvou3\tesseract\tessdata\eng.traineddata`、`PFiles\pinvou3\tesseract\LICENSE`。
- 人工验收：干净 Windows 环境安装后，中英文扫描件 PDF 上传 OCR 验证通过。
