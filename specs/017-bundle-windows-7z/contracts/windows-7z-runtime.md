# 契约：Windows 7z 运行时资源

## 资源来源

用户提供源目录：

```text
C:\Program Files\7-Zip
```

源目录中当前观察到的文件：

```text
7z.exe
7z.dll
7-zip.dll
7-zip32.dll
7zFM.exe
7zG.exe
License.txt
readme.txt
History.txt
Lang\
```

实现阶段可只复制运行压缩包解析所需的最小文件集，但必须保留许可证和来源说明。

## 裁剪后的随附文件

Windows MSI 只应随附以下文件：

```text
7z.exe
7z.dll
License.txt
readme.txt
README.md
```

不得随附以下非运行必需内容：

```text
7zFM.exe
7zG.exe
7-zip.dll
7-zip32.dll
7z.sfx
7zCon.sfx
7-zip.chm
History.txt
Uninstall.exe
Lang\
```

已验证 `7z.exe` + `7z.dll` 单独位于同一目录时，执行 `7z.exe i` 仍显示 `Rar`、`Rar5` 和 `Rar1/2/3/5` 解码器。

`7-zip.dll` 和 `7-zip32.dll` 是 Windows Shell 插件；`7z.sfx` 和 `7zCon.sfx` 是创建自解压包的 SFX 模块。当前项目只通过命令行执行 `7z.exe l -slt` 和 `7z.exe x` 解析压缩包，不做 Shell 集成，也不创建自解压包，因此这些文件不应随 MSI 打包。

## 安装布局

Windows MSI 安装后资源应位于：

```text
{安装目录}\7zip\
```

应用运行时应优先使用：

```text
{安装目录}\7zip\7z.exe
```

OS 层必须封装可执行文件名和安装目录差异，业务层不得感知。

## PATH 策略

- 可以通过 WiX fragment 将 `{安装目录}\7zip` 追加到系统 PATH，保持与 Poppler/Pandoc/Tesseract 一致。
- 应用进程内调用不得只依赖 PATH；应优先使用 OS 层返回的绝对路径。

## 能力验证

实现前必须执行：

```powershell
& "C:\Program Files\7-Zip\7z.exe" i
```

并确认格式列表是否覆盖：

- `zip`
- `7z`
- `rar`

当前验证结果：`C:\Program Files\7-Zip\7z.exe i` 输出已列出 `Rar`、`Rar5` 和 `Rar1/2/3/5` 解码器，满足当前规格对 rar 的要求。

## 许可证与说明

资源目录必须包含 README 或 LICENSE，说明：

- 7-Zip 版本
- 来源路径或上游来源
- 许可证
- 随附文件清单
- 已验证支持格式
- rar 支持状态
