# 数据模型：Windows 内置 7z

## ArchiveRuntime

表示当前平台可用于压缩包解析的运行时工具。

**字段**

- `platform`：`windows`、`linux` 或 `unsupported`
- `tool_path`：实际执行的压缩包工具路径；Windows 优先为安装目录下的内置资源，Linux 为系统命令
- `is_bundled`：是否来自应用随附资源
- `exists`：运行时是否可用
- `supported_formats`：实现验证时确认的格式集合，必须覆盖规格范围内的 zip、rar、7z

**验证规则**

- Windows 下 `exists` 为 true 时，`tool_path` 必须指向随安装目录分发的可执行文件或明确 fallback。
- Linux 下 `is_bundled` 必须为 false，依赖系统 `7z`。
- Windows runtime 必须通过 zip、rar、7z 样本解析验证。

## ArchiveDependencyStatus

表示依赖体检中压缩包能力的用户可见状态。

**字段**

- `key`：固定为 `archive`
- `installed`：压缩包解析能力是否可用
- `install_hint`：缺失时展示的安装或修复提示
- `visible`：该平台是否展示此依赖体检项

**验证规则**

- Windows 内置资源可用时，`visible` 应为 false 或展示为无需安装，不得提示 `p7zip-full`。
- Linux 缺少 `7z` 时，`visible` 应为 true，`install_hint` 保持 `p7zip-full`。
- Windows 内置资源缺失或损坏时，错误信息应指向修复/重新安装 pinvou，而不是 Linux 包名。

## ArchiveIngestRequest

表示用户上传的压缩包附件解析请求。

**字段**

- `path`：用户选择的压缩包路径
- `basename`：压缩包文件名
- `format`：根据扩展名识别的 `zip`、`rar` 或 `7z`
- `byte_size`：压缩包原始大小

**验证规则**

- 仅当前支持范围内的 zip、rar、7z 进入 archive 解析路径。
- 损坏、空包、密码保护或超限文件应返回 warning，不应造成应用崩溃。

## ArchiveIngestResult

表示压缩包解析后的用户可见结果。

**字段**

- `kind`：固定为 `archive`
- `markdown`：压缩包内可识别文件的汇总内容
- `warning`：失败或部分失败时的可理解提示
- `entry_count`：预检或解析得到的条目数量
- `expanded_bytes`：预检得到的解压后总大小估算

**状态转换**

- `pending` → `prechecked`：运行列表预检并确认未超过安全限制
- `prechecked` → `extracted`：解压到临时目录
- `extracted` → `summarized`：递归解析内部文件并生成 markdown
- `pending/prechecked` → `failed`：工具不可用、格式损坏、密码保护或安全限制触发

## WindowsBundledResource

表示随 Windows MSI 分发的 7z 资源。

**字段**

- `source_path`：构建前的源目录，当前为 `C:\Program Files\7-Zip`
- `install_dir`：安装后的相对目录，计划为 `{安装目录}/7zip`
- `files`：裁剪后的随附文件清单，预期包含 `7z.exe`、`7z.dll`、`License.txt`、`readme.txt`、`README.md`
- `license_notice`：许可证和来源说明

**验证规则**

- 文件清单必须包含可执行工具及其运行所需 DLL，不得包含 `7-zip.dll`、`7-zip32.dll`、`7zFM.exe`、`7zG.exe`、`7z.sfx`、`7zCon.sfx`、`7-zip.chm`、`Uninstall.exe`、`Lang/` 等非 CLI 解析必需内容。
- README 或 LICENSE 必须说明来源、版本、能力边界和许可证信息。
- 源包能力验证必须确认 `7z.exe i` 输出包含 `Rar` 和 `Rar5`。
