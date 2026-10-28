# webpki-roots 兼容包装层的变更控制

RustSec 在 2026-10-21 修订了 [RUSTSEC-2026-0381](https://github.com/RustSec/advisory-db/blob/main/crates/webpki-roots/RUSTSEC-2026-0381.md)
及 [RUSTSEC-2026-0383](https://github.com/RustSec/advisory-db/blob/main/crates/webpki-root-certs/RUSTSEC-2026-0383.md)，
显式将各自的 `0.26.11` 兼容包装层列为 `unaffected`。本仓的 advisories 扫描对这两个公告
没有任何忽略项，其他版本仍正常接受扫描，包括包含独立旧根数据的 `webpki-roots 0.26.10`。
在守卫落地之前，这份锁图没有专门的变更控制：`webpki-roots` 的 1.x 根数据曾随常规依赖
清理从 `1.0.10` 漂移到 `1.0.11`（当前图，见 #777），没有门禁要求重新评估。本 PR 把当前
图登记为钉定基线并加上机器强制；此后任何相关版本更新都必须经由此守卫重新钉定。

已审核的 `webpki-roots 0.26.11` 是兼容包装层：发布归档中的 `Cargo.toml` 唯一正常依赖是
`parent = { package = "webpki-roots", version = "1" }`，`src/lib.rs` 仅重导出
`parent::TLS_SERVER_ROOTS`，没有独立根证书数据或构建脚本。三份锁文件现均将
该依赖解析到 `1.0.11`，保留现有 FastEmbed / ORT 运行时（含 Windows 支持），
无需为消除包装层版本告警升级运行时主版本。

`scripts/guard-webpki-roots-exception.py` 在不联网、不加载 submodule 的情况下，
同时核对 Knowledge、桌面应用和 CLI 的三份锁文件。只接受官方 crates.io 来源、
唯一的 `0.26.11` 与 `1.0.11` 包、基线校验值以及精确依赖边：

| 版本 | 发布归档 SHA-256 |
| --- | --- |
| 0.26.11 | `521bc38abb08001b01866da9f51eb7c5d647a19260e00054a8c7fd5f9e57f7a9` |
| 1.0.11 | `f5f280bbb2b287bc8e7de7e3852945a0ff50d1bbcb5ed42b6c7a568c546df171` |

`1.0.11` 是本守卫落地时社区仓库记录的钉定基线：这是变更控制基线，不是一次新的
安全评审；`1.0.11` 的公告状态仍由保持开启的 advisories 扫描独立给出。缺失或损坏
锁文件、降级、新版本、异源包、校验值变化、重复包和依赖边变化都会拒绝。未来任何
相关版本更新必须重新核对发布归档、公告修复范围与依赖图，更新守卫及回归；不能只
扩大允许范围。这里的机器强制面仅为上述两个 `webpki-roots` 包的版本、来源、校验值
及依赖边；`webpki-root-certs`、`rustls-webpki`、`rustls` 的修复版本由锁文件记录；
Knowledge 与桌面主锁有 CI 公告扫描，CLI 目前只有下文的本地组合扫描命令，没有独立
CI deny 步骤。此守卫没有固定所有 TLS 包。主锁之外的测试 harness 锁和 CodeWhale
submodule 锁不在本守卫覆盖范围，不能把主锁验证当作它们的扫描结果。
包装层消失或版本改变时应重新评审，并移除或更新专用守卫，不能直接跳过。

**裸 `cargo deny` 不会执行上述图变更守卫**，但会正常扫描这些公告。
CI 的两个共享策略入口（`knowledge-rust` 与 `rust-lint`）在同一个失败即停止的步骤内
先执行守卫，再用 `--locked` 扫描，避免元数据解析修改已审查的锁文件。另有 bans 规则
独立拒绝 `webpki-roots >=1.0.0, <1.0.11` 和 `>=0.26.0, <0.26.11`，但它不能取代完整
图守卫。两处扫描步骤显式使用 [GitHub 内置 `shell: bash`](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#jobsjob_idstepsshell)（`bash --noprofile --norc -eo pipefail {0}`），
不依赖 workflow/job 的 shell 默认值；同时显式固定仓库根与桌面 crate 的工作目录。
契约测试拒绝删除或替换这两个步骤的 shell 和工作目录。Draft PR 不运行这两个 job
（`rust-lint` 另受 `RUN_HEAVY_RUST_CHECKS` 门控）；Ready PR、Merge Queue 与 main
push 才执行相应检查。

本地需 Python 3.11+ 和 cargo-deny 0.20.2（与 CI 钉定版本一致）。从仓库根运行以下
组合命令；守卫失败时必须停止，不得直接执行下一条或新增全局忽略：

```sh
python3 scripts/guard-webpki-roots-exception.py && cargo deny --locked --manifest-path pinvou-knowledge/Cargo.toml --config pinvou3-app/src-tauri/deny.toml check advisories licenses bans sources
python3 scripts/guard-webpki-roots-exception.py && cargo deny --locked --manifest-path pinvou3-app/src-tauri/Cargo.toml --config pinvou3-app/src-tauri/deny.toml check advisories licenses bans sources
python3 scripts/guard-webpki-roots-exception.py && cargo deny --locked --manifest-path pinvou-cli/Cargo.toml --config pinvou3-app/src-tauri/deny.toml check advisories licenses bans sources
```

在不支持 `&&` 的旧 PowerShell 中，先检查守卫的 `$LASTEXITCODE` 为 `0`，再执行
对应扫描。回归命令为 `python3 -m unittest discover -s scripts/tests -p 'test_webpki_roots_exception.py'`；
fast-gate 的 `Gate policy test suite` 步骤会随全量 `test_*.py` 一并执行。
其他公告、许可证、来源和依赖策略均继续按既有规则执行；此变更控制不表示产品的
所有安全风险均已消除，也不替代后续依赖升级。CI 契约检查器是有限的工作流方言检查，
并非完整 YAML/shell 解析器。它支持普通 `run:` 键及明确识别的标量；显式拒绝 step flow mapping、
引号 run/uses 键、冒号前空格、多行 plain 标量，以及带块头注释、缩进标记、tag/anchor 的 run 标量。
新增 action、本地 composite action 或 reusable workflow 必须扩展显式审核清单；已登记的远程
action 内部实现仍由其自身维护，检查器不递归审计 action 源码。其他未列出的 YAML 语义不作封闭性保证。
对已识别的标量，检查器按 shell 单词去除引号和转义并识别命令分隔符，
因此 `'cargo' 'deny'`、相邻引号、续行及直呼 `cargo-deny` 不会漏掉新增扫描。该静态检查不执行 shell，
也不声称能求值变量、`eval` 或外部脚本；引入动态审计入口时必须显式扩展入口契约。
