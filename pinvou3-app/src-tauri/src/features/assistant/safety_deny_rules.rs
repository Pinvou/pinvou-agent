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
//! 底座 embedder 通道 `EngineConfig.exec_policy_engine` 是原生注入点：引擎在
//! 每次工具调用前（先于审批、先于 hook）做 token 级 + shell 展开匹配，typed
//! `Deny` 短路于一切审批模式（含 YOLO/Never），且天然覆盖 hook 够不着的嵌套
//! 子代理。本会话已有先例：`scope_deny_ruleset`（连接器/技能门禁）走同一通道。
//!
//! 匹配语义（`crates/execpolicy`）：
//! - `command` 型 deny 规则被提升进 `denied_prefixes`（deny-always-wins）；
//! - `deny_scan_targets` 对命令做 shell 展开（剥 sudo/doas/env/nohup/timeout/
//!   xargs 等 16 种 wrapper、引号/命令替换/链式分段），所以 `command = "sudo"`
//!   一条规则即覆盖 `sudo rm`、`/usr/bin/sudo`、`sudo -u root …`、链式段等
//!   全部变体；
//! - 规则工具名 `exec_shell` 经 `canonical_action_alias` 匹配 `Bash` 家族
//!   （`action:"run"`）与 `exec_shell` 旧拼写。
//!
//! ## v1 语义（迁移原 hook 第 1-4 段的意图）
//!
//! | 原段 | 迁移形态 |
//! |---|---|
//! | 1. SENSITIVE_DIRS 路径子串（全工具 ARGS） | `read`/`grep`/`list` × 目录变体 + `find` `-path`/`-ipath` 遍历守卫 |
//! | 2. SENSITIVE_NAMES 文件名子串 | `read`/`grep` × 文件名变体 |
//! | 3. DANGEROUS_CMDS（原已失效） | `cat`/`less`/`more`/`head`/`tail` × 敏感文件 + `ssh-keygen`/`gpg --export-secret-keys` 命令词 |
//! | 4. 超级权限关闭态拦 sudo（原已失效） | `sudo`（+`sudoedit`）命令词 deny，规则集按 `super_permission::is_enabled()` 快照状态增删 |
//!
//! 两个细节：
//! - 只发只读查看器（`cat`/`less`/…）。原 hook 是全 ARGS 子串匹配（写也会被
//!   误伤），但命令前缀通道做子串/`find`/tee 守卫的误拦面太大（模型经常把
//!   `~/.aws` 目录整体传给构建工具），v1 收敛到「密钥/凭证的读取泄露」。
//! - 规则 1/2/4 集合固定、规则 3 补齐原 hook 漏掉的 `/etc/sudoers.d/`，面不
//!   小于原 hook（deny 面宁可保守不可放宽）。
//!
//! ## v1 已知语义差异（详见 PR 描述）
//!
//! - Bash 命令体按 token 前缀匹配，覆盖不了 `xxd /etc/shadow` 这类冷门查看器
//!   /非查看器转写（原 hook 的宽子串反之会误拦一切含 `credentials` 的命令）；
//! - `File` 工具路径规则受工作区归一化限制，家目录绝对路径不生成规则（原 hook
//!   曾以子串形态覆盖 File 调用）；
//! - 规则 4 是规则集构建时的状态快照：中途切换超级权限开关的会话内热刷依赖
//!   `set_super_permission` 触发 `refresh_permission_rulesets`，与既有
//!   scope 规则同口径。

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

/// 只读文本查看器：第 1/2/3 段共用。原 hook 事实上只拦得住查看泄露
/// （DANGEROUS_CMDS 全是 cat/gpg 读取形态），这里显式化并扩到常用变体。
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
/// 同时发两种拼写，子路径命中由 `sensitive_name_read_rules` 的文件级规则补。
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
fn sudo_block_rules() -> Vec<ToolAskRule> {
    sudo_block_rules_for(crate::platform::super_permission::is_enabled())
}

/// [`sudo_block_rules`] 的两态可注入形态（测试用）：关闭态生成 sudo/sudoedit
/// deny，开启态（NOPASSWD 免密，sudo 不阻塞）不生成。
fn sudo_block_rules_for(enabled: bool) -> Vec<ToolAskRule> {
    if enabled {
        return Vec::new();
    }
    vec![
        deny_cmd("sudo".to_string()),
        deny_cmd("sudoedit".to_string()),
    ]
}

/// `find` 目录遍历守卫：规则 1/2 覆盖「查看器直接读路径」形态，但
/// `find -path` 把敏感目录整个吐出来是同级别的泄露。`find` 的常规用法
/// （`-name`/`-type` 等）不受影响——`-path`/`-ipath` 是必须显式传的谓词。
/// 覆盖常见搜索根（~/$HOME/./..//）与敏感目录本体两种形态。
fn find_traversal_rules() -> Vec<ToolAskRule> {
    const SEARCH_ROOTS: &[&str] = &["~", "$HOME", ".", "..", "/"];
    let mut rules = Vec::new();
    for root in SEARCH_ROOTS {
        for flag in ["-path", "-ipath"] {
            rules.push(deny_cmd(format!("find {root} {flag}")));
        }
    }
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
/// 生成不了可匹配规则，v1 只对「敏感目录名 + `/`」的相对路径形态发规则——
/// 会话工作区里的 `.ssh/`、`credentials` 等同名文件仍被硬拒。Bash 命令体里
/// 的家目录路径由上面的命令规则覆盖。这是 v1 的已知语义差异（原 hook 以
/// ARGS 子串覆盖 File 调用），登记在 PR 描述。
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
    let mut rules = sensitive_dir_read_rules();
    rules.extend(sensitive_name_read_rules());
    rules.extend(dangerous_command_rules());
    rules.extend(find_traversal_rules());
    rules.extend(file_tool_path_rules());
    rules.extend(sudo_block_rules());
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

/// 仅供测试/调试：规则集的 `Ruleset` 形态。
#[cfg(test)]
pub(crate) fn safety_deny_ruleset() -> codewhale_execpolicy::Ruleset {
    ruleset_with_denied_prefix_promotion(safety_deny_rules())
}

#[cfg(test)]
mod tests {
    use super::*;
    use codewhale_execpolicy::{AskForApproval, ExecPolicyContext, ExecPolicyEngine};

    fn engine() -> ExecPolicyEngine {
        ExecPolicyEngine::with_rulesets(vec![safety_deny_ruleset()])
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

    fn sudo_rules_present() -> bool {
        // macOS/Windows 测试机上超级权限恒为关闭态 → sudo 规则必然存在。
        // （CI 只有 mac/linux runner；linux 上 /etc/sudoers.d/pinvou3 不存在。）
        sudo_block_rules().len() == 2
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
        let rules = safety_deny_rules();
        // 规则总数按段核对：目录 10×2 家拼写×5 查看器 + find 10×2×2 + 文件名 8×2×5
        // + 绝对路径(5-1 家)… 快照断言用总量 + 关键成员，避免脆断言到逐条。
        let expected_min = 100;
        assert!(
            rules.len() >= expected_min,
            "规则集明显小于迁移面: {}",
            rules.len()
        );
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
            "find ~ -path",
            "find . -ipath",
            "find $HOME/.ssh -path",
        ] {
            // 前缀规则核对：`head -n 5 ~/.gnupg/x` 类带 flag/参数形态由
            // 目录前缀规则覆盖（flag 感知 + 参数位 token 匹配）。
            assert!(commands.contains(&must), "缺少关键规则前缀: {must}");
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
        // sudo 两态。
        if sudo_rules_present() {
            assert!(commands.contains(&"sudo"));
            assert!(commands.contains(&"sudoedit"));
        }
    }

    #[test]
    fn sudo_deny_covers_wrapper_and_path_spellings() {
        if !sudo_rules_present() {
            return; // 超级权限开启态（linux 真机开了免密）不生成 sudo 规则。
        }
        let engine = engine();
        for cmd in [
            "sudo rm -rf /tmp/x",
            "/usr/bin/sudo id",
            "sudo -u root cat /etc/passwd",
            "echo hi && sudo apt install x",
            "sudo bash -c 'whoami'",
            "sudoedit /etc/hosts",
        ] {
            let d = check(&engine, cmd);
            assert!(!d.allow, "sudo 拦截应覆盖: {cmd}");
        }
        // 词边界：不含 sudo 的命令不误伤。
        assert!(check(&engine, "ls -la").allow);
        assert!(check(&engine, "echo sudoers-lecture").allow);
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
            // find 遍历泄露。
            "find ~ -path '*.ssh*' -print",
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
            "ssh user@host",
            "git status",
            "echo credentials-rotation-guide",
            "cat docs/id_rsa-rotation.md",
        ] {
            let d = check(&engine, cmd);
            assert!(d.allow, "不应误拦: {cmd} -> {:?}", d.reason());
        }
    }
}
