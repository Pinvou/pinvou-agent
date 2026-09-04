# 发布产物体积与调试信息策略

本文档是「二进制链接 + 打包」体积优化的单一参考：各产物用什么压缩、哪些 flag
钉在哪、以及为什么某些选项被刻意排除。策略性断言由
`scripts/tests/test_release_size_policy.py`（CI 于 pr-check 的 release-contract
job 执行）钉扎，改 flag 必须同步改该测试。

## 优化杠杆总览（2026-09 起）

| 环节 | 机制 | 钉扎位置 |
|---|---|---|
| Rust 全图优化 | `opt-level=3` + `lto="thin"` + `codegen-units=1` | `pinvou3-app/src-tauri/Cargo.toml [profile.release]` |
| Thin LTO 执行者 | **rustc 自己**(最终单元 `-C lto=thin`;依赖拿 `-C linker-plugin-lto` 产出纯 bitcode 供 rustc 读取)。三端(Apple ld / MSVC link.exe / lld)均真实生效,**与链接器身份无关**。Linux 注入 lld 是为了 link 内存(OOM 实证)与 ICF,不是为了 LTO | Cargo.toml 注释;cargo 1.98 verbose 实证 |
| 死代码消除 | rustc 对 GNU/ELF 默认传 `--gc-sections`；macOS 默认传 `-dead_strip` | rustc 内建，无需配置 |
| 相同代码折叠（ICF，语义统一、按链接器各自表达） | Linux:lld `--icf=safe`(RUSTFLAGS 注入);Windows:rustc 在 opt-level>0 时自动传 `/OPT:REF,ICF`(与 `--icf` 同类,均为链接器 ICF);macOS:Apple ld/ld-prime **无 ICF 能力**,需换 lld-MachO 才有,被仓库明确否决(见下) | `release-packages.yml` 两个 Linux job env |
| 调试信息 | `debug=false` + `strip="symbols"`：发布产物零 DWARF/行号表/符号表。MSVC 的 PDB 落在构建目录、从不进安装包；macOS 恢复全版本统一 strip（见下） | `[profile.release]`；行号表只保留在 `release-fast`（CI 冒烟用，不发布） |
| 构建机路径 | `--remap-path-prefix=<workspace>=/`（Linux + macOS 发布 job 的 RUSTFLAGS） | `release-packages.yml` |
| deb 压缩 | data.tar 从 tauri 硬编码的 gzip-6 重打包为 xz -9（`scripts/repack-deb-xz.sh`，`--root-owner-group` 保证 root 属主；control.tar 同为 xz） | `release-packages.yml` 两个 Linux job + `scripts/release-deb.sh` |
| DMG 压缩 | tauri 产出 UDZO 未设 zlib-level（hdiutil 默认级别 1），构建后转 **ULMO**（LZMA，需 macOS 10.15+ 挂载；本项目最低 11.0） | `release-packages.yml` macOS job + `scripts/release-macos.sh` |
| NSIS 压缩 | 已是 solid LZMA（`config/platforms/windows/tauri.conf.json` 的 `bundle.windows.nsis.compression`，tauri v2 默认即最优档） | 无需改动 |
| 前端资产 | tauri `compression`（default feature）以 Brotli q9 压缩嵌入二进制的 frontendDist | tauri 内建，无更强档位 |
| Windows OTA zip | `CompressionLevel::SmallestSize`（原 Optimal） | `scripts/build-windows-ota.ps1` |
| knowledge-server ELF | 独立构建的 deb 内 ELF 对齐主应用：thin LTO + strip | `pinvou-knowledge/Cargo.toml [profile.release]` |

## macOS strip 与 dyld 对齐修复

曾因 macOS 27 dyld 新增 LINKEDIT 字符串池 8 字节对齐校验、而 rustc 1.96/1.97 的
strip 产物未对齐（proc-macro dylib dlopen 被拒 → E0463），CI 在 macOS 27+ runner
注入 `strip=none` 规避。该 bug 已由 LLVM 修复并 backport 进 **rustc 1.98.0**
（rust-lang/rust#157750、#158410；工具链版本钉在
`pinvou3-app/src-tauri/rust-toolchain.toml`），两个 workflow 的规避 step 已删除，
macOS 产物恢复与 Linux/Windows 一致的零符号表状态。

## 需要调试符号时（crash 符号化）

发布产物刻意零调试信息；需要符号化时的独立调试工件（均不入任何发布产物）：

- macOS：`[profile.release]` 临时加 `split-debuginfo = "packed"`（`debug` 需非
  false），rustc 产出 `.dSYM`；
- Linux：`split-debuginfo = "packed"` 产出 `.dwp`（stable）；
- Windows：PDB 恒生成于 `target/`（rustc 对 MSVC 无条件传 `/DEBUG`），直接取用。

## 刻意排除的选项（及原因）

| 选项 | 原因 |
|---|---|
| `panic = "abort"`（省 5-10%） | 底座 CodeWhale 用 `catch_unwind` 做「单次工具 panic 不拖垮会话」的隔离，属设计约束 |
| macOS/Windows 换用 lld 以「统一 ICF flag」 | 语义上三端折叠策略已统一(见总览表),flag 字面不同只是各链接器的表达:lld=`--icf=safe`、link.exe=`/OPT:REF,ICF`(rustc 自动)。为字面统一而换链接器不划算:macOS 换 lld-MachO 要处理 Tauri framework/签名链(`.cargo/config.toml` 头注释已否决),Windows 换 lld-link 动摇 WebView2/COM 的稳定链接路径;且 lld 并非 thin LTO 的前提(thin LTO 由 rustc 执行,见总览表) |
| `trim-paths`（cargo profile） | rustc/cargo 1.98 上仍为 nightly-only（cargo#12137 未稳定）；已用 stable 的 `--remap-path-prefix` 达成同等的产物路径清理 |
| `build.removeUnusedCommands`（tauri ≥2.4 体积项） | 依赖 ACL 枚举命令；本应用 capabilities 只引用 core/dialog 权限、自定义命令全靠默认放行，启用会剪掉全部自定义命令 |
| `include_dir` 压缩 | include_dir 0.7.4 无 compression feature；~10MB 内嵌技能资源如需压缩属自定义代码改造，另行立项 |
| MSI / AppImage / rpm 压缩档 | MSI 与 AppImage、rpm 当前不在发布矩阵（仅 deb/dmg/nsis）；rpm 有 `bundle.linux.rpm.compression`（zstd）旋钮，未来启用 rpm 时应直接设 zstd |
| fat LTO（替代 thin） | ubuntu 16GB runner 上 link 内存越线被 SIGTERM（历史 run 实证），thin 为既定取舍 |

## 已知边界

- Developer ID 正式签名的 DMG（私有发布管道）如在签名**后**做容器格式转换会使
  DMG 自身签名失效，需重签；本仓库社区链路（ad-hoc，`signingIdentity="-"`）的
  dmg 本身不签名，不受影响。
- `RUSTFLAGS` env 优先级高于 `.cargo/config.toml` 的 rustflags：Linux/macOS 发布
  job 的 flag 全部走 env 注入，`.cargo/config.toml` 刻意保持零硬编码（跨平台
  链接器可得性，见该文件头注释）。
- deb 重打包用 `dpkg-deb`，仅 Linux 可用；两处调用方（CI job、release-deb.sh）
  均为 Linux 环境。
