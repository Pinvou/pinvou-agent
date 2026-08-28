//! 敏感数据/提权硬拦截规则集（原 bundle hook `deny_sensitive_paths.sh/.ps1`
//! 第 1-4 段的迁移落点，v1）。
//!
//! ## 背景：hook 为什么失效
//!
//! 底座 v0.9.3 起模型/执行面只暴露 `Bash` 工具（`exec_shell*` 拼写进入
//! `RETIRED_TOOL_NAMES`），hook 收到的是模型原始调用名 `Bash`；hook 脚本第
//! 3/4 段（DANGEROUS_CMDS / sudo 拦截）按 `$TOOL == "exec_shell"*` 门控，
//! 因此静默失效（`exit 0` 放行）。本次不修 hook 的工具名匹配，直接把策略
//! 迁入底座 execpolicy 规则引擎。
//!
//! ## 为什么选 EngineConfig.exec_policy_engine（程序化注入）
//!
//! 底座 embedder 通道 `EngineConfig.exec_policy_engine` 是原生注入点：引擎对
//! 主线会话的每次工具调用做 token 级 + shell 展开/剥壳匹配，typed `Deny`
//! 短路于一切审批模式（含 YOLO/Never）。求值点在 ToolCallBefore hook 之后、
//! 审批之前（两道防线任一命中都会拦，互不依赖）。注意覆盖面以主线会话为界：
//! 嵌套子代理的工具调用不经过本检查（底座子代理执行器未接 execpolicy，见
//! 已知差异）。本会话已有先例：`scope_deny_ruleset`（连接器/技能门禁）走
//! 同一通道。
//!
//! 匹配语义（`crates/execpolicy`）：
//! - `command` 型 deny 规则被提升进 `denied_prefixes`（deny-always-wins）；
//! - `deny_scan_targets` 对命令做 shell 展开（剥 sudo/doas/env/nohup/timeout/
//!   xargs 等 18 种 wrapper、引号/命令替换/链式分段），所以 `command = "sudo"`
//!   一条规则即覆盖 `sudo rm`、`/usr/bin/sudo`、`sudo -u root …`、链式段等
//!   全部变体；
//! - 规则工具名 `exec_shell` 经 `canonical_action_alias` 匹配 `Bash` 家族
//!   （`action:"run"`）与 `exec_shell` 旧拼写。
//!
//! ## v1 语义（迁移原 hook 第 1-4 段的意图）
//!
//! | 原段 | 迁移形态 |
//! |---|---|
//! | 1. SENSITIVE_DIRS 路径子串（全工具 ARGS） | `read`/`grep`/`list` × 目录变体 + `find <敏感目录>` `-path`/`-ipath` 遍历守卫 |
//! | 2. SENSITIVE_NAMES 文件名子串 | `read`/`grep` × 文件名变体 |
//! | 3. DANGEROUS_CMDS（原已失效） | `cat`/`less`/`more`/`head`/`tail` × 敏感文件 + `ssh-keygen`/`gpg --export-secret-keys` 命令词 |
//! | 4. 超级权限关闭态拦 sudo（原已失效） | `sudo`（+`sudoedit`）命令词 deny，规则集按 `super_permission::is_enabled()` 快照状态增删 |
//!
//! 两个细节：
//! - 只发只读查看器（`cat`/`less`/…）。原 hook 是全 ARGS 子串匹配，活着的
//!   第 1/2 段连写/转写向量（`cp`/`tar`/`rsync` 触碰敏感目录）也一并拦；命令
//!   前缀通道做这类守卫的误拦面太大（模型经常把 `~/.aws` 目录整体传给构建
//!   工具），v1 有意收敛到「密钥/凭证的读取泄露」——整体 deny 面由此小于
//!   原 hook，收窄项全部登记在下一节，不做无披露的静默放宽。
//! - 复活面：规则 3（含补齐原 hook 漏掉的 `/etc/sudoers.d/`）与规则 4 原本
//!   已静默失效，迁移后重新生效；规则 1/2 在「查看器读取」子面上不小于
//!   原 hook 的同形态命中。
//!
//! ## v1 已知语义差异（详见 PR 描述）
//!
//! - Bash 命令体按 token 前缀匹配，覆盖不了 `xxd /etc/shadow` 这类冷门查看器
//!   /非查看器转写（原 hook 的宽子串反之会误拦一切含 `credentials` 的命令）；
//! - 写/转写/外传向量不覆盖：`cp`/`tar`/`rsync`/`scp` 触碰敏感目录原被第
//!   1/2 段子串拦，v1 无对应规则；
//! - 非 `~`/`$HOME` 前缀的绝对路径不覆盖：`cat /root/.ssh/id_rsa`、其他用户
//!   家目录形态（原 hook 子串 `/.ssh/` 可拦）；
//! - 敏感目录的子路径文件仅部分覆盖：文件级规则只发 `~/.ssh` 五个键名与
//!   `~/.aws/credentials`，`cat ~/.kube/config`、`~/.docker/config.json`、
//!   Chrome Cookies 等不再拦（原 hook 子串可拦）；
//! - 非 Bash 工具面不覆盖：原第 1/2 段对所有工具的 ARGS 子串生效（fetch/
//!   rlm/tasks/Git/MCP 等），v1 只键到 `exec_shell` 与 File 读族；
//! - Windows 面未迁移：原 `.ps1` 第 1/2 段的 `%appdata%\microsoft\credentials`
//!   /`protect` 等路径拼写与凭证命令词（`cmdkey`/`vaultcmd`/`get-credential`
//!   等）无对应规则。Windows 上 Bash 工具默认经 pwsh/cmd 等系统登录 shell
//!   执行，模型写
//!   Windows 原生拼写时不命中（POSIX 拼写的规则仍是纯文本命中）；
//! - 嵌套子代理的工具调用不经过 execpolicy（见上），YOLO 下子代理不受本
//!   规则约束——待底座在子代理执行器接入检查后闭合；
//! - `File` 工具路径规则受工作区归一化限制，家目录绝对路径不生成规则（原
//!   hook 曾以子串形态覆盖 File 调用）；
//! - 目录列举/元数据命令不覆盖：`ls`/`stat`/`tree`/`du`/`file` 触碰敏感目录
//!   放行——原第 1 段尾斜杠子串（`/.ssh/`）拦 `ls ~/.ssh/` 等列举形态，文件名
//!   枚举本身可泄露密钥存在性（future work：列举类查看器）；
//! - 敏感目录作搜索根的非 `-path` find 形态不覆盖：`find ~/.ssh/ -name id_rsa`
//!   原被尾斜杠子串拦，v1 只守 `-path`/`-ipath`（无尾斜杠拼写 `find ~/.ssh
//!   -name` 原 hook 本就不拦——子串要求尾斜杠，两版一致）；
//! - heredoc/多行命令体可能过拦：底座段级扫描按真实换行切段（宁可过拦取向），
//!   写有 `cat /etc/shadow` 字面行的脚本/文档会被硬拒（底座 deny_scan 固有
//!   行为，规则 3 复活后开始命中）；
//! - 规则 4 是规则集构建时的状态快照：中途切换超级权限开关的会话内热刷依赖
//!   `set_super_permission` 触发 `refresh_permission_rulesets`，与既有
//!   scope 规则同口径。开关命令未串行化，并发连打存在窄窗口的陈旧快照，
//!   以最终一次写盘后的任一次重算/引擎重启为准。

use codewhale_execpolicy::{PermissionAction, ToolAskRule};

/// 原 hook 第 1 段 SENSITIVE_DIRS 的目录名（POSIX 侧）。
const SENSITIVE_DIR_NAMES: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".docker",
    ".kube",
    ".config/google-chrome",
    ".mozilla/firefox",
    ".password-store",
    ".dws",
    ".tmeet",
];

/// 原 hook 第 2 段 SENSITIVE_NAMES 的文件名。
/// 原 hook 第 2 段 SENSITIVE_NAMES 的文件名（shell 规则与 File 工具路径
/// 规则共用；File 侧按工作区相对路径精确匹配）。
const SENSITIVE_FILE_NAMES: &[&str] = &[
    "id_rsa",
    "id_ed25519",
    "id_ecdsa",
    "id_dsa",
    "authorized_keys",
    "credentials",
    "secrets",
    ".pgp",
    ".gpg",
    ".netrc",
    ".git-credentials",
];

/// 原 hook 第 3 段 DANGEROUS_CMDS 对应的敏感文件绝对路径（`~` 由
/// `home_dir_variants` 在调用处展开）。
const SENSITIVE_ABS_FILES: &[&str] = &[
    "~/.ssh/",            // read ~/.ssh/ 目录枚举（原 hook: "cat ~/.ssh"）
    "~/.aws/credentials", // 原 hook: "cat ~/.aws/credentials"
    "/etc/shadow",        // 原 hook: "cat /etc/shadow"
    "/etc/sudoers",       // 原 hook: "cat /etc/sudoers"
    "/etc/sudoers.d/",    // 补齐：原 hook 漏掉的 sudoers.d 目录
];

/// 只读文本查看器：第 1/2/3 段共用。原第 3 段全是 cat/gpg 读取形态；活着的
/// 第 1/2 段子串拦得更宽（含写/转写向量，见模块注释已知差异），这里显式化
/// 只读查看器并扩到常用变体。
/// 不含 `grep`：其路径在参数位（`grep PATTERN path`），命令前缀通道无法
/// 表达（已知差异，PR 登记）。
const READ_VIEWERS: &[&str] = &["cat", "less", "more", "head", "tail"];

/// `File` 工具的读取/搜索 action（`canonical_action_alias` 解析后的规则工具名：
/// `File` 家族的 read/list/search_name/search_content → read_file/list_dir/
/// file_search/grep_files）。
const FILE_READ_ACTIONS: &[&str] = &["read_file", "list_dir", "file_search", "grep_files"];

/// `~` 的两种展开拼写（模型两种都会写）。
fn home_dir_variants() -> [String; 2] {
    ["~/".to_string(), "$HOME/".to_string()]
}

/// `command` 型 deny 规则（工具 = exec_shell，覆盖 Bash 家族）。
fn deny_cmd(command: String) -> ToolAskRule {
    let mut rule = ToolAskRule::exec_shell(command);
    rule.action = PermissionAction::Deny;
    rule
}

/// `path` 型 deny 规则（规则工具名 = `canonical_action_alias` 解析值）。
fn deny_file_path(tool: &str, path: String) -> ToolAskRule {
    let mut rule = ToolAskRule::file_path(tool, path);
    rule.action = PermissionAction::Deny;
    rule
}

/// 家目录下敏感路径的完整拼写变体（`~/.ssh/…`、`$HOME/.ssh/…`）。
fn home_sensitive_path(dir_or_file: &str) -> Vec<String> {
    let rel = dir_or_file.strip_prefix("~/").unwrap_or(dir_or_file);
    home_dir_variants()
        .into_iter()
        .map(|home| format!("{home}{rel}"))
        .collect()
}

/// 一个敏感路径（目录形态）的「查看器读取」规则族。
///
/// 底座参数位是**精确 token 匹配**：`cat ~/.ssh` 不匹配 `cat ~/.ssh/`（带尾
/// 斜杠的目录拼写），也不匹配 `cat ~/.ssh/id_rsa`（子路径）。因此目录规则要
/// 同时发两种拼写；子路径（`cat ~/.ssh/id_rsa`）仅当文件名在
/// `sensitive_name_read_rules` 清单内才命中（目前 `~/.ssh` 五个键名 +
/// `~/.aws/credentials`），其余敏感目录的子路径文件（`~/.kube/config` 等）
/// 是已知缺口（模块注释已登记）。
fn viewer_rules_for(path_variants: &[String]) -> Vec<ToolAskRule> {
    let mut rules = Vec::new();
    for path in path_variants {
        for viewer in READ_VIEWERS {
            rules.push(deny_cmd(format!("{viewer} {path}")));
        }
    }
    rules
}

/// 规则 1：敏感目录的读取（Bash 侧，目录参数两种拼写 × 查看器）。
fn sensitive_dir_read_rules() -> Vec<ToolAskRule> {
    let mut rules = Vec::new();
    for dir in SENSITIVE_DIR_NAMES {
        // 目录两种尾斜杠拼写（~ 与 $HOME 两种家前缀 ×2）。
        let mut variants = home_sensitive_path(dir);
        for variant in home_sensitive_path(&format!("{dir}/")) {
            variants.push(variant);
        }
        rules.extend(viewer_rules_for(&variants));
    }
    rules
}

/// 规则 2：敏感文件名的读取（Bash 侧）。
///
/// 原第 2 段是全 ARGS 子串匹配（任何位置出现即拒）；命令规则通道只能按
/// 「查看器 + 路径 token」表达。v1 收敛为文件级规则：每个文件名在其
/// 所在敏感目录 + 家根目录下生成完整路径（`~/.ssh/id_rsa`、`~/credentials`）。
/// 任意深度的同名文件（`~/project/secrets`）不再被子串误拦，也不被覆盖——
/// 已知差异，PR 登记。
fn sensitive_name_read_rules() -> Vec<ToolAskRule> {
    // 文件名 → 所在目录（`~/` = 家根）。
    let name_dirs: &[(&str, &str)] = &[
        ("id_rsa", "~/.ssh/"),
        ("id_ed25519", "~/.ssh/"),
        ("id_ecdsa", "~/.ssh/"),
        ("id_dsa", "~/.ssh/"),
        ("authorized_keys", "~/.ssh/"),
        ("credentials", "~/"),
        ("secrets", "~/"),
        (".pgp", "~/"),
        (".gpg", "~/"),
        (".netrc", "~/"),
        (".git-credentials", "~/"),
    ];
    let mut rules = Vec::new();
    for (name, dir) in name_dirs {
        let variants: Vec<String> = home_dir_variants()
            .into_iter()
            .map(|home| format!("{home}{}{name}", dir.strip_prefix("~/").unwrap_or("")))
            .collect();
        rules.extend(viewer_rules_for(&variants));
    }
    rules
}

/// 规则 3：敏感绝对路径的查看器读取 + ssh-keygen/gpg 导出命令词。
fn dangerous_command_rules() -> Vec<ToolAskRule> {
    let mut rules = Vec::new();
    for file in SENSITIVE_ABS_FILES {
        // 目录路径两种尾斜杠拼写；文件路径单拼写（`~`/`$HOME` ×2）。
        let mut variants: Vec<String> = if let Some(rest) = file.strip_prefix("~/") {
            home_dir_variants()
                .into_iter()
                .flat_map(|home| {
                    let full = format!("{home}{rest}");
                    if full.ends_with('/') {
                        let bare = full.trim_end_matches('/').to_string();
                        vec![bare, full]
                    } else {
                        vec![full]
                    }
                })
                .collect()
        } else if file.ends_with('/') {
            vec![file.trim_end_matches('/').to_string(), file.to_string()]
        } else {
            vec![file.to_string()]
        };
        variants.dedup();
        rules.extend(viewer_rules_for(&variants));
    }
    // 命令词级 deny：原 hook 第 3 段的 ssh-keygen / gpg --export-secret。
    // gpg 规则在 `gpg` 后带 `-k` 等无关 flag 时被参数位 token 匹配挡住，
    // 但 `gpg --export-secret-keys` 的正常拼写全部命中（flag 感知跳过）。
    rules.push(deny_cmd("ssh-keygen".to_string()));
    rules.push(deny_cmd("gpg --export-secret-keys".to_string()));
    rules.push(deny_cmd("gpg --export-secret-subkeys".to_string()));
    rules
}

/// 规则 4：超级权限关闭态的 sudo 硬拒。
///
/// 源真相 = `/etc/sudoers.d/pinvou3` 是否存在（`super_permission::is_enabled`
/// 实时读盘，macOS/Windows 恒 false）。规则集构建时快照状态。`sudo` 单命令词
/// 经底座 deny-scan 覆盖 `/usr/bin/sudo`、`sudo -u root …`、`sudo bash -c …`、
/// 链式段等全部变体；sudoedit 同理。开启态不生成（sudo 免密直跑，不拦）。
///
/// macOS/Windows 恒关闭态即恒拦：平台本不支持开关（turn_reminder 引导用户在
/// 终端手跑 root 命令），自配 NOPASSWD sudoers 的 macOS 用户同样被拦——与
/// 「平台不支持超级权限」的产品口径一致，属有意的口径收敛。deny 理由是底座
/// 通用文案（丢失原 hook 的开关引导文案），由每 turn 的 turn_reminder 补偿。
///
/// [`sudo_block_rules_for`] 是其两态可注入形态（测试/桥接回归注入固定状态，
/// 不读宿主盘）：关闭态生成 sudo/sudoedit deny，开启态（NOPASSWD 免密，sudo
/// 不阻塞）不生成。
fn sudo_block_rules_for(enabled: bool) -> Vec<ToolAskRule> {
    if enabled {
        return Vec::new();
    }
    vec![
        deny_cmd("sudo".to_string()),
        deny_cmd("sudoedit".to_string()),
    ]
}

/// `find` 目录遍历守卫：只守「敏感目录本身作为搜索根 + `-path`/`-ipath`」
/// 形态。相对原 hook 的同形态（尾斜杠子串拦 `find ~/.ssh/ -path …`）只增不减；
/// 但原尾斜杠子串还拦 `-name` 等其他 find 形态与尾斜杠目录列举（`ls ~/.ssh/`），
/// v1 不覆盖——无尾斜杠拼写（`find ~/.ssh -name`）原 hook 本就不拦。收窄项
/// 登记在模块注释已知差异。
///
/// 刻意不发「通用搜索根 + `-path`」前缀规则（`find . -path`、`find / -path`
/// 等）：denied_prefixes 是 token 前缀匹配，`find . -path ./x -prune` 与
/// `find . -not -path '…'` 这类 find 标准排除惯用法会被确定性硬拒，且 typed
/// Deny 无审批出路，误拦面远大于泄露面。通用搜索根下的名字发现（
/// `find ~ -name id_rsa`）与 grep 参数位同属 token 通道表达边界，登记为
/// future work。
fn find_traversal_rules() -> Vec<ToolAskRule> {
    let mut rules = Vec::new();
    for dir in SENSITIVE_DIR_NAMES {
        let rel = dir.strip_prefix("~/").unwrap_or(dir);
        for base in home_dir_variants() {
            let path = format!("{base}{rel}");
            for flag in ["-path", "-ipath"] {
                rules.push(deny_cmd(format!("find {path} {flag}")));
            }
        }
    }
    rules
}

/// `File` 工具（canonical `File` 家族，`read`/`grep`/`list` action）的路径
/// 规则。
///
/// 工作区归一化只接受工作区内路径：家目录绝对路径（`~/.ssh` 的真实展开）
/// 生成不了可匹配规则，v1 只对敏感目录/文件名的**工作区根相对路径**发规则
/// （path 匹配是归一化后的精确相等）——工作区根下的同名文件/目录（
/// `id_rsa`、`.ssh/`）仍被硬拒；嵌套相对路径（`docs/secrets/`）按精确相等
/// 不命中。Bash 命令体里的家目录路径由上面的命令规则覆盖。这是 v1 的已知
/// 语义差异（原 hook 以 ARGS 子串覆盖 File 调用），登记在 PR 描述。
fn file_tool_path_rules() -> Vec<ToolAskRule> {
    let mut rules = Vec::new();
    for name in SENSITIVE_FILE_NAMES {
        for action in FILE_READ_ACTIONS {
            rules.push(deny_file_path(action, name.to_string()));
        }
    }
    // 敏感目录相对路径形态（`.ssh` 等）：list_dir 匹配目录读取；文件级
    // read/grep 前缀无法用相等比较表达，文件名规则已按所在目录补齐。
    for dir in SENSITIVE_DIR_NAMES {
        let rel = dir.strip_prefix("~/").unwrap_or(dir);
        rules.push(deny_file_path("list_dir", rel.to_string()));
    }
    rules
}

/// 敏感数据/提权硬拦截规则集（v1）。
///
/// spawn 注入初值（`build_engine_config_for_session_roots`）与超级权限开关
/// 切换后的热刷（`EnginePool::refresh_permission_rulesets`）共用这一份计算。
/// 调用方（bridge）把它并入 scope 门禁规则集的同一 `Ruleset`。
#[must_use]
pub fn safety_deny_rules() -> Vec<ToolAskRule> {
    safety_deny_rules_for(crate::platform::super_permission::is_enabled())
}

/// [`safety_deny_rules`] 的超级权限两态可注入形态：`enabled=true`（NOPASSWD
/// 免密直跑）不生成 sudo 规则。生产路径读盘快照；测试注入固定状态，避免
/// 宿主机 `/etc/sudoers.d/pinvou3` 的真实状态影响可重复性。
pub(crate) fn safety_deny_rules_for(super_permission_enabled: bool) -> Vec<ToolAskRule> {
    let mut rules = sensitive_dir_read_rules();
    rules.extend(sensitive_name_read_rules());
    rules.extend(dangerous_command_rules());
    rules.extend(find_traversal_rules());
    rules.extend(file_tool_path_rules());
    rules.extend(sudo_block_rules_for(super_permission_enabled));
    rules
}

/// 把 typed Deny 规则集同时提升进 `denied_prefixes`（底座 config 加载器
/// `PermissionsToml::ruleset()` 的同一语义）。
///
/// 只放 `ask_rules` 时命令走 `allow_rule_matches`：纯前缀比对、无 flag 跳过、
/// 无命令词 basename 折叠——`sudo` 规则拦不住 `/usr/bin/sudo`，`cat /etc/shadow`
/// 拦不住 `head -n 5 /etc/shadow`。`denied_prefixes` 通道（deny-always-wins）
/// 才有 flag 感知 + basename 折叠 + wrapper 剥离（`deny_scan_targets`）。
/// 提升 = deny 面不小于原 hook 的词边界正则，两条通道并存取并集。
///
/// 与底座 config 加载器（`PermissionsToml::ruleset()`）的唯一不对称：这里
/// trusted 恒空、只提升 Deny。当前输入全为 typed Deny，产物与加载器逐字段
/// 等价；若未来混入 Allow 规则，会静默丢失加载器给 Allow 的 trusted_prefix
/// 提升（Passive 方向，偏保守不放大 deny 面）——届时应对齐加载器把 Allow
/// 也提进 trusted。
pub(crate) fn ruleset_with_denied_prefix_promotion(
    rules: Vec<ToolAskRule>,
) -> codewhale_execpolicy::Ruleset {
    let denied = rules
        .iter()
        .filter(|r| r.action == PermissionAction::Deny)
        .filter(|r| !r.command_exact && r.workspace.is_none())
        .filter_map(|r| r.command.clone())
        .collect::<Vec<_>>();
    codewhale_execpolicy::Ruleset::user(vec![], denied).with_ask_rules(rules)
}

/// 仅供调试：规则集的 `Ruleset` 形态。
#[cfg(test)]
pub(crate) fn safety_deny_ruleset_with_state(
    super_permission_enabled: bool,
) -> codewhale_execpolicy::Ruleset {
    ruleset_with_denied_prefix_promotion(safety_deny_rules_for(super_permission_enabled))
}

#[cfg(test)]
mod tests {
    use super::*;
    use codewhale_execpolicy::{AskForApproval, ExecPolicyContext, ExecPolicyEngine};

    fn engine() -> ExecPolicyEngine {
        // 注入「关闭态」而非读宿主盘：真机开了免密（/etc/sudoers.d/pinvou3
        // 存在）时 sudo 规则不生成，测试必须与宿主状态解耦才可重复。
        ExecPolicyEngine::with_rulesets(vec![safety_deny_ruleset_with_state(false)])
    }

    fn check(engine: &ExecPolicyEngine, command: &str) -> codewhale_execpolicy::ExecPolicyDecision {
        engine
            .check(ExecPolicyContext {
                command,
                cwd: ".",
                tool: Some("exec_shell"),
                path: None,
                ask_for_approval: AskForApproval::Never,
                sandbox_mode: None,
            })
            .unwrap()
    }

    /// sudo 两态规则快照：关闭态生成 sudo/sudoedit deny；开启态（NOPASSWD）
    /// 规则集完全不含 sudo（放行）。状态注入自 `sudo_block_rules_for`。
    #[test]
    fn sudo_rules_snapshot_both_states() {
        let disabled = sudo_block_rules_for(false);
        assert_eq!(disabled.len(), 2);
        let commands: Vec<&str> = disabled
            .iter()
            .filter_map(|r| r.command.as_deref())
            .collect();
        assert!(commands.contains(&"sudo"));
        assert!(commands.contains(&"sudoedit"));

        let enabled = sudo_block_rules_for(true);
        assert!(
            enabled.is_empty(),
            "超级权限开启态不应生成任何 sudo deny 规则"
        );
        // 并入完整规则集后的两态差异（规则集构建时快照语义）。
        let with_disabled = ruleset_with_denied_prefix_promotion(vec![deny_cmd("sudo".into())]);
        assert!(with_disabled.denied_prefixes.iter().any(|p| p == "sudo"));
        let with_enabled = ruleset_with_denied_prefix_promotion(sudo_block_rules_for(true));
        assert!(with_enabled.denied_prefixes.is_empty());
    }

    #[test]
    fn rule_snapshot_is_stable() {
        let rules = safety_deny_rules_for(false);
        // 规则总数按段核对（关闭态）：目录 10×4 拼写×5 查看器=200 + 文件名
        // 11×2×5=110 + 绝对路径 10 拼写×5+3 命令=53 + find 敏感目录根 10×2×2=40
        // + File 11×4+10=54 + sudo 2 = 459。精确计数：整段被删/被旁路时立刻红
        // （>=100 类弱断言允许静默丢 ~78%）。注意 459 含 20 条跨段重复——规则 1
        // 的 `.ssh` 目录 4 变体与规则 3 的 `~/.ssh/` 绝对路径条目展开出完全相同
        // 的 20 条命令（`cat ~/.ssh` 等 ×5 查看器），dedup 只在段内做；若未来加
        // 跨段去重，计数降到 439 属预期，需同步更新本断言。
        assert_eq!(rules.len(), 459, "规则集总量漂移：确认是有意增删后更新计数");
        let commands: Vec<&str> = rules.iter().filter_map(|r| r.command.as_deref()).collect();
        for must in [
            "cat ~/.ssh/",
            "cat $HOME/.ssh/",
            "cat ~/.ssh/id_rsa",
            "cat ~/.aws/credentials",
            "cat ~/credentials",
            "cat ~/.git-credentials",
            "cat /etc/shadow",
            "cat /etc/sudoers",
            "cat /etc/sudoers.d/",
            "less /etc/shadow",
            "head ~/.gnupg/",
            "ssh-keygen",
            "gpg --export-secret-keys",
            "cat ~/.password-store/",
            "cat ~/.dws/",
            "cat ~/.tmeet/",
            "find ~/.ssh -path",
            "find ~/.ssh -ipath",
            "find $HOME/.ssh -path",
        ] {
            // 前缀规则核对：`head -n 5 ~/.gnupg/x` 类带 flag/参数形态由
            // 目录前缀规则覆盖（flag 感知 + 参数位 token 匹配）。
            assert!(commands.contains(&must), "缺少关键规则前缀: {must}");
        }
        // 通用搜索根的 find -path 前缀规则必须不存在：会确定性硬拒 find 的
        // 标准排除惯用法（-path X -prune / -not -path）。
        for must_not in [
            "find ~ -path",
            "find . -path",
            "find . -ipath",
            "find / -path",
        ] {
            assert!(
                !commands.iter().any(|c| c.starts_with(must_not)),
                "不应有通用根 find 规则: {must_not}"
            );
        }
        // File 工具路径规则存在（工具名 = canonical read/grep/list）。
        let file_rules = rules
            .iter()
            .filter(|r| r.path.is_some())
            .map(|r| (r.tool.as_str(), r.path.as_deref().unwrap()))
            .collect::<Vec<_>>();
        for (tool, path) in [
            ("read_file", "id_rsa"),
            ("grep_files", "credentials"),
            ("list_dir", ".ssh"),
        ] {
            assert!(
                file_rules.contains(&(tool, path)),
                "缺少 File 路径规则 {tool} {path}"
            );
        }
        // 关闭态 sudo 两规则常驻（注入态，不依赖宿主盘）。
        assert!(commands.contains(&"sudo"));
        assert!(commands.contains(&"sudoedit"));
    }

    #[test]
    fn sudo_deny_covers_wrapper_and_path_spellings() {
        let engine = engine();
        for cmd in [
            "sudo rm -rf /tmp/x",
            "/usr/bin/sudo id",
            "sudo -u root cat /etc/passwd",
            "echo hi && sudo apt install x",
            "sudo bash -c 'whoami'",
            // 自省形态同样被拦（与原 hook 第 4 段词边界正则同口径）。
            "sudo -l",
            "sudoedit /etc/hosts",
        ] {
            let d = check(&engine, cmd);
            assert!(!d.allow, "sudo 拦截应覆盖: {cmd}");
        }
        // 词边界：不含 sudo 的命令不误伤。
        assert!(check(&engine, "ls -la").allow);
        assert!(check(&engine, "echo sudoers-lecture").allow);
    }

    /// 开启态（NOPASSWD 免密）完整规则集不含 sudo 拦截：`sudo`/`sudoedit`
    /// 引擎级放行。锁定规则 4 的两态快照语义在引擎层的表现。
    #[test]
    fn super_permission_enabled_ruleset_allows_sudo() {
        let engine = ExecPolicyEngine::with_rulesets(vec![safety_deny_ruleset_with_state(true)]);
        for cmd in ["sudo -l", "sudo apt update", "sudoedit /etc/hosts"] {
            let d = check(&engine, cmd);
            assert!(d.allow, "开启态不应拦: {cmd} -> {:?}", d.reason());
        }
    }

    #[test]
    fn sensitive_shell_reads_are_denied_across_spellings() {
        let engine = engine();
        for cmd in [
            // 原 hook 第 3 段实测失效路径（Bash + cat /etc/shadow）——证伪原 bug 已修复。
            "cat /etc/shadow",
            "cat /etc/sudoers",
            "cat ~/.ssh/id_rsa",
            "cat $HOME/.ssh/authorized_keys",
            "cat ~/.aws/credentials",
            // 链式 / 引号 / wrapper 变体。
            "echo hi && cat /etc/shadow",
            "cat \"/etc/shadow\"",
            "cat '/etc/shadow'",
            "bash -c 'cat ~/.ssh/id_rsa'",
            "less /etc/shadow",
            "head -n 5 /etc/sudoers",
            "tail /etc/shadow",
            // ssh-keygen / gpg 导出。
            "ssh-keygen -t ed25519",
            "gpg --export-secret-keys me",
            "gpg --armor --export-secret-keys me",
            "gpg --export-secret-subkeys me",
            // find 遍历泄露（敏感目录作为搜索根）。
            "find ~/.ssh -path '*id_rsa*' -print",
            "find $HOME/.ssh -ipath '*id_rsa*'",
            // 家目录下的 SENSITIVE_NAMES（原第 2 段）。
            "cat ~/.netrc",
            "cat $HOME/.git-credentials",
            // 敏感目录列表读取（原第 1 段）。
            "cat ~/.gnupg/",
            "cat ~/.kube/",
            "cat ~/.config/google-chrome/",
            "cat ~/.mozilla/firefox/",
            "cat ~/.password-store/",
        ] {
            let d = check(&engine, cmd);
            assert!(!d.allow, "应被 deny: {cmd} -> {:?}", d.reason());
        }
    }

    #[test]
    fn ordinary_commands_are_not_over_denied() {
        let engine = engine();
        for cmd in [
            "cat README.md",
            "cat src/main.rs",
            "less package.json",
            "head Cargo.toml",
            "find . -name '*.rs'",
            "find . -type f",
            // find 的标准排除惯用法（-path 前缀规则的已知误拦形态）必须放行。
            "find . -path ./node_modules -prune -o -type f -print",
            "find / -path /proc -prune -o -name '*.log' -print",
            "find . -not -path './node_modules/*' -type f",
            "ssh user@host",
            "git status",
            "echo credentials-rotation-guide",
            "cat docs/id_rsa-rotation.md",
            // 已登记收窄的 allow 留痕（模块注释「v1 已知语义差异」）：这些形态
            // 原 hook 宽子串会拦、v1 有意放行，锁定防止未来被无意识拦回而不红。
            "cp ~/.ssh/id_rsa /tmp/x", // 写/外传向量
            "cat /root/.ssh/id_rsa",   // 非 ~/$HOME 前缀绝对路径
            "cat ~/.kube/config",      // 敏感目录子路径文件
            "ls ~/.aws/",              // 目录列举/元数据命令
        ] {
            let d = check(&engine, cmd);
            assert!(d.allow, "不应误拦: {cmd} -> {:?}", d.reason());
        }
    }
}
