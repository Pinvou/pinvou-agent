use std::path::{Component, Path, PathBuf};

use codewhale_execpolicy::sensitive_paths;

/// Resolve an artifact path against its execution workspace without imposing
/// command-layer knowledge on feature code.
pub(crate) fn resolve_artifact_path_in_workspace(raw: &str, workspace: &Path) -> String {
    if Path::new(raw).is_absolute() {
        raw.to_string()
    } else {
        workspace.join(raw).to_string_lossy().into_owned()
    }
}

/// 凭据和系统敏感组件黑名单——上传/摄入路径不得包含这些组件。
///
/// Wave 3 收敛：原先 `validate_browsable_path`（file_ingest）只挡 5 个敏感
/// **目录**（.ssh/.gnupg/.aws/.docker/.kube），挡不住 `~/keys/id_rsa`、
/// `~/config/.env`、`~/config/credentials.json` 等非目录凭据文件。此黑名单
/// 与 `validate_user_path` 共享同一份，确保所有用户路径入口校验一致。
/// `pub` for the pinvou-cli `check_sensitive_path` mirror (artifacts.rs): the
/// CLI imports these lists instead of a hand copy, so an upstream credential
/// addition can never silently reopen the CLI surface — the drift the copy's
/// own doc warned about.
///
/// 引擎清单单源（CodeWhale #87 `sensitive_paths` 模块）：本清单是引擎
/// `SENSITIVE_DIRECTORY_NAMES` ∪ `SENSITIVE_FILE_NAMES` ∪
/// `SENSITIVE_COMPONENT_NAMES` 中 Pinvou 摄入门所需子集的登记，下方 `const`
/// 断言把每项锚定在引擎清单上——引擎侧改名/删除会在编译期失败并强制同步，
/// 不再靠注释对齐。刻意的引擎残差（不登记）：
/// 多段目录名（`.config/gcloud` 等——本闸按「任意单个路径组件相等」比较，
/// 多段字面量永不命中；对应敏感面由引擎 ruleset 按 `~/` 相对拼写承担）、
/// `.azure`/`.dws`/`.tmeet` 目录、以及仅 home 根敏感的裸文件名
/// （`credentials`/`secrets`/`.pgp`/`.gpg`/`.netrc`/`.git-credentials`——
/// 组件面登记会误拦同名工作目录）。`id_rsa`/`id_ed25519`/`id_ecdsa`/`id_dsa`
/// 已迁移为引擎组件面派生（`ID_COMPONENT_KEYS`：#87 评审轮给引擎补上裸
/// `id_*` 组件面后，按常量上的预登记迁移点改为从
/// `SENSITIVE_COMPONENT_NAMES` 编译期锚定；Wave 3 动机即 `~/keys/id_rsa`）。
pub const BLOCKED_COMPONENTS: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".docker",
    ".kube",
    ".password-store",
    "id_rsa",
    "id_ed25519",
    "id_ecdsa",
    "id_dsa",
    "credentials.json",
    ".env",
    // 与 `.env` 同类的本地凭据/密钥文件,逐一枚举的黑名单此前漏掉的常見
    // 命名(direnv 的 .envrc 常含导出的密钥;.git-credentials/.netrc/
    // .pgpass/.npmrc 是各工具的默认凭据存储)。
    ".envrc",
    ".git-credentials",
    ".netrc",
    ".pgpass",
    ".npmrc",
    // Round-9 复审 minor:同类默认凭据存储——pip 的 `.pypirc`(上传
    // token)、HashiCorp Vault 的 `.vault-token`(裸 token 文件)、git 的
    // `http.cookiefile`(`.gitcookies`,o/t 形态令牌)、cargo 的
    // `credentials.toml`(registry 凭据,与既有 `credentials.json` 同型)。
    ".pypirc",
    ".vault-token",
    ".gitcookies",
    "credentials.toml",
    // Round-8 复审 MAJOR:仓库自身的 .git 内部是手机可达工作区车道里最
    // 常见的凭据存储(.git/config 的远端 URL 常内嵌 oauth2:TOKEN@ 形态的
    // 凭据;FETCH_HEAD 同理),而 `.git-credentials` 这个旧式存储早被拦。
    // 精确组件匹配只拦名字恰为 `.git` 的组件,`.gitignore`/`.github`/
    // `.gitattributes` 等不同组件不受影响。按 code_checkpoints 的双清单
    // 纪律:SECRET_EXCLUDES 走 gitignore 语义(.git 本就被 git 忽略),
    // 两侧独立评估、互不推导。
    ".git",
    // Round-11 复审 MAJOR:云 CLI 凭据存储与 `.aws` 同级(项目自身的
    // safety_deny_rules v3.3 清单早已把它们列为同 tier):gcloud 的
    // ADC 刷新令牌 `application_default_credentials.json` 与 SQLite 库
    // `credentials.db`(注意组件精确匹配下既有 `credentials.json` 条目
    // 命不中更长的文件名)、Azure CLI 的 `msal_token_cache.json`/
    // `accessTokens.json` 令牌缓存,以及目录名 `.azure`;`gcloud` 作为
    // 裸目录名偏宽(`$HOME/.config/gcloud` 是标准位置),但模型上下文
    // 车道对示例/误伤宁可过宽的既定标准(`.env.example` 被拦同理)。
    // Round-11 复审 minor 同批:其余 distinctive 命名的默认凭据存储——
    // s3cmd `.s3cfg`、MySQL `.my.cnf`、msmtp `.msmtprc`、rclone
    // `rclone.conf`、Terraform `credentials.tfrc.json`。浏览器
    // Cookies/Login Data、gh hosts.yml、Maven settings.xml、sops
    // age keys.txt 等通用命名不进全局精确清单(会误伤任意工作区里的
    // 同名普通文件),待目录作用域匹配能力再收(残余记录)。
    "application_default_credentials.json",
    "credentials.db",
    "msal_token_cache.json",
    "accessTokens.json",
    ".azure",
    "gcloud",
    ".s3cfg",
    ".my.cnf",
    ".msmtprc",
    "rclone.conf",
    "credentials.tfrc.json",
    // Round-12 复审 P2:同族补遗——OpenSSH FIDO2 硬件密钥的默认私钥名
    // `id_ed25519_sk`/`id_ecdsa_sk`(精确条目 `id_ed25519`/`id_ecdsa` 命不
    // 中更长的派生名,与 gcloud ADC 命不中 `credentials.json` 同型的缺口);
    // git 全局配置 `.gitconfig`(`[credential]` 段与 `insteadOf` 改写 URL
    // 可内嵌令牌,与既有 `.git-credentials` 同类存储)。
    "id_ed25519_sk",
    "id_ecdsa_sk",
    ".gitconfig",
    // Round-14 复审:与既有条目同型的默认凭据存储补遗——Yarn Berry
    // `.yarnrc.yml`(npmAuthToken)与经典 `.yarnrc`(registry 认证,
    // `.npmrc` 同型)、Emacs `.authinfo`/`.authinfo.gpg`(netrc 明文格式,
    // `.netrc` 同型)、wget/curl 的 `.wgetrc`/`.curlrc`(`http_password`/
    // `-u user:pass`)与 Windows 命名 `_curlrc`/`_netrc`(被拦 `.netrc`
    // 的 Windows 对位名)、Mercurial `.hgrc`(`[auth]` 段明文,
    // `.gitconfig` 同型)、gsutil/boto `.boto`(AWS 密钥,`.s3cfg`
    // 同型)、minikube 目录(`ca.key`/per-profile admin `client.key`,
    // `.kube` 同型)、Android ADB 私钥 `adbkey`(可认证任意已授权设备,
    // 含无线调试)。
    ".yarnrc.yml",
    ".yarnrc",
    ".authinfo",
    ".authinfo.gpg",
    ".wgetrc",
    ".curlrc",
    "_curlrc",
    "_netrc",
    ".hgrc",
    ".boto",
    ".minikube",
    "adbkey",
];

/// 目录作用域凭据文件:末分量是通用文件名(settings.xml/hosts.yml/
/// Local State 这类名字在任意工作区里都是无辜的普通文件),只有落在
/// 特定凭据目录序列之下才拦——按「路径以固定分量序列结尾」判定,每
/// 个分量经平台感知的 `path_component_eq`(与 `BLOCKED_COMPONENTS` 同
/// 折叠语义)。清单纪律:序列必须以真实凭据目录开头,不收裸文件名;
/// 浏览器 profile 目录(Cookies/Login Data,名字不定)下的条目中间
/// 分量无法固定,不进本清单(已记录残余)。 Round-11 复审 minor:
/// `.config/gh/hosts.yml` 是 GitHub CLI 的 OAuth token 存储,
/// `.m2/settings.xml` 常内嵌明文 server 密码(`.npmrc` 同型),
/// `.config/sops/age/keys.txt` 是 sops 的年龄私钥,`.config/
/// {google-chrome,chromium}/Local State` 内嵌 Chrome 的加密密钥原料。
/// Round-14 复审 MAJOR/同批补遗:`.config/git/credentials` 是
/// `credential.helper store` 的另一个默认落点(`~/.git-credentials`
/// 的同格式兄弟路径,`https://user:token@host` 明文),`.config/git/
/// config` 与被拦 `.gitconfig` 同类(`[credential]` 段/`insteadOf`
/// 改写内嵌令牌);`.gradle/gradle.properties` 常含发布密钥与仓库
/// PAT(`.m2/settings.xml` 的构建工具对位);`.config/containers/
/// auth.json` 是 Podman 的 registry 凭据存储(`.docker` 同型);
/// `.android/debug.keystore` 是调试签名密钥(口令为公开默认值)。
pub const BLOCKED_PATH_TAILS: &[&[&str]] = &[
    &[".config", "gh", "hosts.yml"],
    &[".m2", "settings.xml"],
    &[".config", "sops", "age", "keys.txt"],
    &[".config", "google-chrome", "Local State"],
    &[".config", "chromium", "Local State"],
    &[".config", "git", "credentials"],
    &[".config", "git", "config"],
    &[".gradle", "gradle.properties"],
    &[".config", "containers", "auth.json"],
    &[".android", "debug.keystore"],
];

/// 目录作用域尾序列命中判定(含 CLI 镜像在内的所有消费方共用);命中时
/// 返回命中的尾序列,供错误文案拼接。
pub fn blocked_path_tail_hit(canonical: &Path) -> Option<&'static [&'static str]> {
    let components: Vec<_> = canonical.components().collect();
    BLOCKED_PATH_TAILS.iter().copied().find(|tail| {
        components.len() >= tail.len()
            && components[components.len() - tail.len()..]
                .iter()
                .zip(tail.iter())
                .all(|(component, expected)| {
                    crate::platform::os::path_component_eq(component.as_os_str(), expected)
                })
    })
}

/// 组件前缀黑名单:`.env.` 系命名(`.env.local`、`.env.production`、
/// `.env.development`……)与 `.env` 同为本地环境凭据文件,环境变体命名
/// 无法逐一枚举,按前缀判定。匹配经由平台感知的 `path_component_prefix`
/// (macOS/Windows Unicode 大小写折叠,与 `path_component_eq` 同语义:
/// ASCII 快路径 + U+017F/U+212A 简单大小写折叠,Windows/macOS 另含
/// NTFS `$UpCase` 大写表的 U+0131 映射)。`.env.example`/`.env.sample`
/// 因此同样被拦,且是有意为之(Round-9 复审 minor):示例文件常被人把
/// 真实密钥忘在里面,模型上下文车道宁可过宽;快照车道(`code_checkpoints`
/// 的 SECRET_EXCLUDES,gitignore 语义)对示例文件/公钥的相反判断是双清
/// 单独立评估的既定差异,两侧互不推导。
pub const BLOCKED_COMPONENT_PREFIXES: &[&str] = &[".env."];

/// 组件后缀黑名单:`*.env` 系命名(`production.env`、`config.env`、
/// `secrets.env`、`deploy.env`……)与 `.env`/`.env.*` 同为本地环境变量
/// 文件——把名字写在前的形态是 `.env.` 前缀规则的镜像缺口,逐一枚举
/// 同样不可能,按后缀判定(Round-12 复审 P2)。`*.env` 是环境变量文件的
/// 通用命名约定,「示例文件宁可过宽」的既定标准与 `BLOCKED_COMPONENT_
/// PREFIXES` 一致(`sample.env` 一并被拦,有意为之);`*.env.bak` 等
/// 二次后缀形态不在此列,与 id_* 备份家族同归「目录作用域匹配」残余。
/// 匹配经由平台感知的 `path_component_suffix`(折叠语义与
/// `path_component_eq`/`path_component_prefix` 完全一致)。
pub const BLOCKED_COMPONENT_SUFFIXES: &[&str] = &[".env"];

/// 系统敏感前缀黑名单——Unix 系统文件/虚拟文件系统路径。
///
/// 引擎清单单源：本清单 ⊆ 引擎 `SENSITIVE_ABSOLUTE_PREFIXES`（const 断言
/// 锚定）。引擎残差（`/etc/shadow-`、`/etc/shadow.bak`、`/etc/gshadow-`、
/// `/etc/sudoers-`、`/etc/sudoers.bak`、`/etc/sudoers.d/`、`/var/log/auth.log`）
/// 是编辑器/老化备份、碎片目录与轮变体的精确词面——引擎 ruleset 的 token 面
/// 需要逐一枚举，本闸 `starts_with` 语义下已被 `/etc/shadow`、`/etc/gshadow`、
/// `/etc/sudoers`、`/var/log/auth` 前缀覆盖，不重复登记。
pub const BLOCKED_PREFIXES: &[&str] = &[
    "/etc/shadow",
    // Round-9 复审 MAJOR:组件化比较把字节 starts_with 的误伤(`/etc/
    // shadowed` 被拦)修成了组件精确,但清单成员还是字节前缀时代的写法
    // ——真实存在的家族成员被静默放行:`shadow-`/`gshadow-` 是 passwd/
    // useradd 落盘的哈希备份,`/var/log/auth.log` 本体是 `/var/log/auth`
    // 条目真正的目标(Debian/Ubuntu 的认证日志,adm 组默认可读)。
    // Round-10 复审 MAJOR:字节前缀同样覆盖的备份家族(管理员 `cp` 留下
    // 的 `.bak`/`.old`,pacman 的 `.pacnew`,vipw 的 `.lock`,编辑器的
    // `~`)按开放家族恢复——`shadow.*` 形式的末分量组件前缀一并收拢
    // `.d/` 解析片段与 `.tmp` 工作文件,且不会误伤 `sudoersedit` 这类
    // 无点同前缀名;`~` 后缀不含点,单独精确枚举。
    "/etc/shadow-",
    "/etc/gshadow",
    "/etc/gshadow-",
    "/etc/sudoers",
    "/etc/ssh/",
    "/root/",
    "/var/log/auth",
    // 轮转日志(auth.log.1 / auth.log.2.gz / …)与上述备份家族是无法逐
    // 一枚举的开放家族:末分量 `*` 表示该分量按组件前缀匹配(清单自律
    // 仅用于此类条目,语义见 path_starts_with_folded)。Round-14 复审
    // minor:注意两类基形的差别——`auth.log*`/`opasswd*` 的 `*` 紧跟
    // 非点基名,连带拦下 `auth.logx`/`opasswd-notes` 这类无点同前缀名
    // (宁可过宽,面收敛在 /var/log 与 /etc/security 之内);`shadow.*`/
    // `sudoers.*` 的点尾基名才有 `sudoersedit` 类无辜名的保护。两种
    // 基形并存是有意的,轮转家族的真实形态恰为点分(auth.log.N)或
    // 无点前缀(opasswd.old),按各自最宽成员选形。
    "/var/log/auth.log*",
    "/etc/shadow.*",
    "/etc/shadow~",
    "/etc/gshadow.*",
    "/etc/gshadow~",
    "/etc/sudoers.*",
    "/etc/sudoers~",
    "/proc/",
    "/sys/",
    // Round-11 复审 NIT:macOS 的 root 用户主目录(Linux `/root/` 的
    // 对位;普通 app 用户不可读,仅在超级权限场景作为对称纵深)。
    "/var/root/",
    // Round-11 复审 minor:pam_unix 的密码历史文件(旧哈希,与 shadow
    // 同内容级);`*` 收拢 .old/.bak 等轮转备份家族。
    "/etc/security/opasswd*",
];

// ── 引擎清单编译期锚定（与 features/assistant::safety_deny_rules 共用）──────
// const eval 里不能调用 PartialEq（非 const fn），一律按字节比较。

/// 逐字节 `&str` 相等（const eval 用）。
pub(crate) const fn inventory_str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// `inventory` 中是否含 `entry`（const eval 用）。
pub(crate) const fn inventory_contains(inventory: &[&str], entry: &str) -> bool {
    let mut i = 0;
    while i < inventory.len() {
        if inventory_str_eq(inventory[i], entry) {
            return true;
        }
        i += 1;
    }
    false
}

/// `inventory` 中是否存在恰为 `prefix`+`suffix` 拼接的条目（const eval 用；
/// 引擎按 `~/` 相对路径拼写登记文件，本应用按（名字，属主目录）二元组建档）。
pub(crate) const fn inventory_has_joined(inventory: &[&str], prefix: &str, suffix: &str) -> bool {
    let mut i = 0;
    while i < inventory.len() {
        let hay = inventory[i].as_bytes();
        let p = prefix.as_bytes();
        let s = suffix.as_bytes();
        if hay.len() == p.len() + s.len() {
            let mut matched = true;
            let mut k = 0;
            while k < p.len() {
                if hay[k] != p[k] {
                    matched = false;
                    break;
                }
                k += 1;
            }
            let mut k = 0;
            while matched && k < s.len() {
                if hay[p.len() + k] != s[k] {
                    matched = false;
                }
                k += 1;
            }
            if matched {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// 引擎组件面派生的裸 `id_*` 私钥名（预登记迁移已落地）：#87 评审轮给引擎
/// 补上 `SENSITIVE_COMPONENT_NAMES`（含裸 `id_*`）后，原「app 专属组件面 +
/// 互斥断言」按常量上的预登记迁移点退役——四项保留在 `BLOCKED_COMPONENTS`
/// 中，但每项编译期锚定在引擎组件面上：引擎侧改名/删除让本 crate 编译失败，
/// 强制有意识同步；引擎组件面新增项由 forkguard 覆盖测试强制采纳或登记残差。
/// Wave 3 动机不变：`~/keys/id_rsa` 这类任意目录下的密钥文件。
const ID_COMPONENT_KEYS: &[&str] = &["id_rsa", "id_ed25519", "id_ecdsa", "id_dsa"];

const fn all_components_in_engine_inventory(entries: &[&str]) -> bool {
    let mut i = 0;
    while i < entries.len() {
        if !inventory_contains(sensitive_paths::SENSITIVE_DIRECTORY_NAMES, entries[i])
            && !inventory_contains(sensitive_paths::SENSITIVE_FILE_NAMES, entries[i])
            && !inventory_contains(sensitive_paths::SENSITIVE_COMPONENT_NAMES, entries[i])
        {
            return false;
        }
        i += 1;
    }
    true
}

const fn all_entries_in(entries: &[&str], inventory: &[&str]) -> bool {
    let mut i = 0;
    while i < entries.len() {
        if !inventory_contains(inventory, entries[i]) {
            return false;
        }
        i += 1;
    }
    true
}

const fn all_in_engine_prefixes(entries: &[&str]) -> bool {
    let mut i = 0;
    while i < entries.len() {
        if !inventory_contains(sensitive_paths::SENSITIVE_ABSOLUTE_PREFIXES, entries[i]) {
            return false;
        }
        i += 1;
    }
    true
}

// 子集锚定：引擎清单任何一侧的改名/删除让本 crate 编译失败，强制有意识同步。
const _: () = assert!(
    all_components_in_engine_inventory(BLOCKED_COMPONENTS),
    "BLOCKED_COMPONENTS 必须逐项存在于引擎 sensitive_paths 三份清单（改名/删除需同步本子集）"
);
const _: () = assert!(
    all_entries_in(
        ID_COMPONENT_KEYS,
        sensitive_paths::SENSITIVE_COMPONENT_NAMES
    ) && all_entries_in(ID_COMPONENT_KEYS, BLOCKED_COMPONENTS),
    "裸 id_* 必须从引擎 SENSITIVE_COMPONENT_NAMES 派生且保留在 BLOCKED_COMPONENTS"
);
const _: () = assert!(
    all_in_engine_prefixes(BLOCKED_PREFIXES),
    "BLOCKED_PREFIXES 必须逐项存在于引擎 SENSITIVE_ABSOLUTE_PREFIXES"
);

/// macOS 把 `/etc`、`/var`、`/tmp` 符号链接到 `/private/...` 下的真实目录;
/// `std::fs::canonicalize` 会把调用方的 `/etc/...` 拼写改写成解析后的
/// `/private/etc/...`,系统前缀黑名单若只比对改写后的路径就会整体失效
/// (Round-5 复审 M5:`/etc/sudoers` 在 macOS 上永远匹配不到 `/etc/sudoers`,
/// 折叠修复的 `/Etc/Sudoers` 同理只对不存在的路径生效)。比对前剥掉这层
/// 别名;仅 macOS 启用——Linux 上 `/private/etc` 是普通目录,必须保持
/// 字节精确语义,Windows 无此别名。
fn strip_macos_private_alias(path: &Path) -> Option<PathBuf> {
    let mut components = path.components();
    match (components.next(), components.next(), components.next()) {
        (Some(Component::RootDir), Some(private), Some(target))
            if crate::platform::os::path_component_eq(private.as_os_str(), "private")
                && ["etc", "var", "tmp"].iter().any(|name| {
                    crate::platform::os::path_component_eq(target.as_os_str(), name)
                }) =>
        {
            let mut aliased = PathBuf::from("/");
            aliased.push(target);
            aliased.push(components.as_path());
            Some(aliased)
        }
        _ => None,
    }
}

/// 系统前缀命中判定,含 macOS 别名展开。`alias_symlinks` 由调用方按平台
/// 传入(`cfg!(target_os = "macos")`),独立参数让折叠平台语义在所有平台
/// 上都可被单元测试直接驱动。CLI 镜像(`pinvou-cli`)消费同一组合谓词:
/// 系统前缀判定必须与 GUI 完全同源,手抄前缀循环会漏掉别名展开
/// (Round-6 复审 M2)。
pub fn canonical_system_prefix_hit(canonical: &Path, alias_symlinks: bool) -> bool {
    let alias_stripped = if alias_symlinks {
        strip_macos_private_alias(canonical)
    } else {
        None
    };
    BLOCKED_PREFIXES.iter().any(|prefix| {
        crate::platform::os::path_starts_with_folded(canonical, prefix)
            || alias_stripped.as_deref().is_some_and(|aliased| {
                crate::platform::os::path_starts_with_folded(aliased, prefix)
            })
    })
}

/// 检查已规范化路径是否触及凭据组件或系统敏感前缀。
/// 供 `validate_user_path` 和 `validate_browsable_path` 共享。
///
/// 已接受的残余(Round-7 复审核对在案):本判定是「词法 + 解析落点」的
/// 组件策略;同用户在检查与打开之间换掉路径(swap)或硬链接别名
/// (`ln ~/.ssh/id_rsa ~/work/notes.txt` 对组件检查不可见)不在威胁模型
/// 内——本地单用户应用里,这样的行动者本就能直接读该文件;策略意图是
/// 把凭据挡在外部模型上下文之外(见 file_ingest::validate_browsable_path)。
/// 「链接名清白」的符号链接不在残余内:由调用方的 canonicalize + 解析落
/// 点复检封闭(如 codex_acp/workspace 的解析腿、workspace_grants 的
/// dev/ino 身份复核)。
pub(crate) fn check_sensitive_components(canonical: &Path) -> Result<(), String> {
    for blocked in BLOCKED_COMPONENTS {
        if canonical
            .components()
            .any(|component| crate::platform::os::path_component_eq(component.as_os_str(), blocked))
        {
            return Err(format!(
                "path {} crosses sensitive component {}",
                canonical.display(),
                blocked
            ));
        }
    }

    for prefix in BLOCKED_COMPONENT_PREFIXES {
        if canonical.components().any(|component| {
            crate::platform::os::path_component_prefix(component.as_os_str(), prefix)
        }) {
            return Err(format!(
                "path {} crosses sensitive component prefix {}",
                canonical.display(),
                prefix
            ));
        }
    }

    for suffix in BLOCKED_COMPONENT_SUFFIXES {
        if canonical.components().any(|component| {
            crate::platform::os::path_component_suffix(component.as_os_str(), suffix)
        }) {
            return Err(format!(
                "path {} crosses sensitive component suffix {}",
                canonical.display(),
                suffix
            ));
        }
    }

    // Round-11 复审 minor:目录作用域条目(通用文件名只在特定凭据目录
    // 序列之下才拦,清单与语义见 BLOCKED_PATH_TAILS)。
    if blocked_path_tail_hit(canonical).is_some() {
        return Err(format!(
            "path {} crosses a directory-scoped credential file",
            canonical.display()
        ));
    }

    // 组件序列比较走平台折叠语义(macOS 大小写不敏感卷会把 `/Etc/Sudoers`
    // 解析到真文件,字节 starts_with 会让大写变体绕过系统区黑名单),
    // Linux 字节精确语义由 path_component_eq 的平台分支保持;macOS 再叠加
    // /private 符号链接别名展开(见 strip_macos_private_alias)。
    if canonical_system_prefix_hit(canonical, cfg!(target_os = "macos")) {
        return Err(format!(
            "path {} is in system-sensitive area",
            canonical.display()
        ));
    }

    Ok(())
}

/// Validate a user-controlled path before a feature reads or opens it.
///
/// Pinvou3 is a local single-user application, so paths outside the home
/// directory remain valid. Credential locations and system-sensitive paths
/// are rejected to keep their contents out of an external model context.
pub(crate) fn validate_user_path(raw: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return Err(format!("path must be absolute: {raw}"));
    }

    // Round-5 复审:canonicalize 失败(路径不存在)时保留原拼写前先拒绝
    // `..`——组件有界的黑名单比对会把 `a/../b` 的 ParentDir 视为普通不匹
    // 配组件,未规范化拼写在回退分支里不可信,fail-closed。
    let canonical = match std::fs::canonicalize(&path) {
        Ok(canonical) => canonical,
        Err(_) => {
            if path
                .components()
                .any(|component| component == Component::ParentDir)
            {
                return Err(format!(
                    "path must not contain '..' when it cannot be resolved: {raw}"
                ));
            }
            path.clone()
        }
    };
    check_sensitive_components(&canonical)?;
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_artifact_against_workspace() {
        let workspace = Path::new("C:/work/session");
        assert_eq!(
            resolve_artifact_path_in_workspace("output/report.md", workspace),
            workspace
                .join("output/report.md")
                .to_string_lossy()
                .into_owned()
        );
    }

    #[test]
    fn rejects_relative_user_path() {
        assert!(
            validate_user_path("relative/file.txt")
                .unwrap_err()
                .contains("must be absolute")
        );
    }

    #[test]
    fn rejects_sensitive_component() {
        let path = if cfg!(windows) {
            r"C:\Users\tester\.ssh\id_rsa"
        } else {
            "/home/tester/.ssh/id_rsa"
        };
        assert!(
            validate_user_path(path)
                .unwrap_err()
                .contains("sensitive component")
        );
    }

    /// 全黑名单逐项命中：组件比较须经平台感知 `path_component_eq`（Wave 3
    /// 委托后逐字节比较会让 Windows 大写变体绕过）。从常量读取黑名单构造
    /// 路径，避免测试代码内嵌敏感文件名字面量。
    #[test]
    fn rejects_all_blocked_components() {
        for blocked in BLOCKED_COMPONENTS {
            let path = std::env::temp_dir().join(blocked).join("x.txt");
            let r = check_sensitive_components(&path);
            assert!(r.is_err(), "{blocked} 组件应被拦");
        }
    }

    /// 大小写变体语义跟随平台：Windows 与 macOS 默认卷大小写不敏感，黑名单
    /// 大写写法同样必须命中（卷层会把 `.AWS` 解析到磁盘上的 `.aws`，按调用方
    /// 拼写字节比较的词法检查会被绕过，见 posix.rs `path_component_eq`）；
    /// Linux 大小写敏感，大写变体是不同文件，不拦属正确语义。此用例同时锁住
    /// 「比较经由平台感知 helper」这一修复点。
    #[test]
    fn case_variant_semantics_follow_platform() {
        for blocked in BLOCKED_COMPONENTS {
            let upper = blocked.to_uppercase();
            let path = std::env::temp_dir().join(&upper).join("x.txt");
            let r = check_sensitive_components(&path);
            if cfg!(windows) || cfg!(target_os = "macos") {
                assert!(r.is_err(), "{upper} 应被拦");
            } else {
                assert!(r.is_ok(), "Linux 上 {upper} 是不同文件，不应拦");
            }
        }
    }

    /// 大写变体逐项钉住比较语义：大小写不敏感平台上 `.ENV`/`.AWS` 等拼写
    /// 会解析到与原样拼写相同的磁盘文件，词法黑名单必须同样命中；Linux 上
    /// 同一批大写变体是不同文件、放行，而原样拼写仍拦截。
    /// check_sensitive_components 纯组件判定、不触盘，断言的是比较语义本身。
    /// 大小写不敏感卷上 canonicalize 输出的拼写是平台实现细节（macOS
    /// realpath 保留调用方拼写），任何判定都不得依赖它——词法与解析后两次
    /// 判定都经折叠比较，折叠是承载语义的判定，不得改回字节精确比较。
    #[test]
    fn uppercase_credential_variants_follow_platform() {
        let uppercase_variants = [
            ".ENV",
            ".AWS/credentials",
            ".KUBE/config",
            ".DOCKER/config.json",
            ".SSH",
        ];
        for raw in uppercase_variants {
            let path = std::env::temp_dir().join(raw);
            let r = check_sensitive_components(&path);
            if cfg!(windows) || cfg!(target_os = "macos") {
                assert!(r.is_err(), "大小写不敏感平台上 {raw} 应被拦");
            } else {
                assert!(r.is_ok(), "Linux 上 {raw} 是不同文件，不应拦");
            }
        }
        for raw in uppercase_variants {
            let lower = raw.to_lowercase();
            let path = std::env::temp_dir().join(&lower);
            assert!(
                check_sensitive_components(&path).is_err(),
                "原样拼写 {lower} 应被拦"
            );
        }
    }

    #[test]
    fn env_variants_and_credential_files_are_blocked() {
        // `.env.` 系环境变体逐一枚举追不全,按前缀拦截;.envrc 与各工具的
        // 默认凭据文件按精确组件拦截。
        let blocked = [
            ".env.local",
            ".env.production",
            ".env.development",
            // Round-9 复审 minor:`.env.` 前缀对示例文件同样生效是有意之
            // 举(示例文件常被忘进真实密钥;快照车道的相反判断见
            // BLOCKED_COMPONENT_PREFIXES 文档)。
            ".env.example",
            ".envrc",
            ".git-credentials",
            ".netrc",
            ".pgpass",
            ".npmrc",
            ".git",
            // Round-9 复审 minor:同类默认凭据存储,与上面同判据。
            ".pypirc",
            ".vault-token",
            ".gitcookies",
            "credentials.toml",
        ];
        for raw in blocked {
            let path = std::env::temp_dir().join("app").join(raw);
            assert!(check_sensitive_components(&path).is_err(), "{raw} 必须被拦");
        }
        // .git 内部(config 的远端 URL 常内嵌凭据)经嵌套路径同样被拦;
        // 仅共享前缀的 .gitignore/.github/.gitattributes 是不同组件,不受
        // 影响。Round-8 复审 MAJOR。
        let nested = std::env::temp_dir()
            .join("repo")
            .join(".git")
            .join("config");
        assert!(
            check_sensitive_components(&nested).is_err(),
            ".git/config 必须被拦"
        );
        // 前缀仅按组件边界生效:同名前缀的普通文件不受影响。
        for benign in [
            "environment.md",
            "envelope.txt",
            ".environment",
            ".gitignore",
            ".github",
            ".gitattributes",
        ] {
            let path = std::env::temp_dir().join("app").join(benign);
            assert!(
                check_sensitive_components(&path).is_ok(),
                "{benign} 不是凭据文件,不应被拦"
            );
        }
    }

    /// Round-12 复审 P2:`*.env` 后缀家族(`production.env` 等把名字写在
    /// 前的形态)与 `.env.` 前缀家族互为镜像——前缀规则对它们不可见。
    /// `*.env` 是环境变量文件的通用命名约定,`sample.env` 类示例文件一并
    /// 被拦是既定的宁可过宽;折叠变体(大小写不敏感平台上的
    /// `PRODUCTION.ENV`)由 interface 层的全量条目折叠钉覆盖。同批的
    /// FIDO2 密钥名与 `.gitconfig` 走精确组件,由
    /// `rejects_all_blocked_components` / 删除钉自动覆盖。
    #[test]
    fn env_component_suffix_family_is_blocked() {
        for raw in [
            "deploy/production.env",
            "config.env",
            "secrets.env",
            "app/.env",
        ] {
            let path = std::env::temp_dir().join(raw);
            assert!(
                check_sensitive_components(&path).is_err(),
                "{raw} 应被 *.env 后缀规则拦截"
            );
        }
        for benign in ["dotenv.md", "myenv", "env.txt", "notes.environment"] {
            let path = std::env::temp_dir().join(benign);
            assert!(
                check_sensitive_components(&path).is_ok(),
                "{benign} 不是 *.env 环境文件,不应被拦"
            );
        }
    }

    /// Round-11 复审:硬编码字面量钉住原始清单成员(`.gnupg`/`.password-
    /// store`/`id_*` 私钥/`credentials.json`/`/root/`/`/proc/`/`/sys/`)
    /// 与本批新增成员——逐常量循环在成员被删除时静默通过(迭代次数变少
    /// 不红),删除任何一个条目必须在这里变红。含 `credentials.json` 精确
    /// 匹配与 gcloud ADC 长文件名的区分(后者不再漏网)。
    #[test]
    fn original_entries_stay_blocked_when_deleted_from_the_constant() {
        let blocked_components = [
            ".ssh",
            ".gnupg",
            ".aws",
            ".docker",
            ".kube",
            ".password-store",
            "id_rsa",
            "id_ed25519",
            "id_ecdsa",
            "id_dsa",
            "credentials.json",
            // Round-11 复审 MAJOR:云 CLI 凭据家族。
            "application_default_credentials.json",
            "credentials.db",
            "msal_token_cache.json",
            "accessTokens.json",
            ".azure",
            "gcloud",
            ".s3cfg",
            ".my.cnf",
            ".msmtprc",
            "rclone.conf",
            "credentials.tfrc.json",
            // Round-12 复审 P2:FIDO2 硬件密钥默认名与 git 全局配置。
            "id_ed25519_sk",
            "id_ecdsa_sk",
            ".gitconfig",
            // Round-14 复审:同型默认凭据存储补遗(yarn/authinfo/wget/
            // curl/hg/boto/minikube/adb,见清单内注释)。
            ".yarnrc.yml",
            ".yarnrc",
            ".authinfo",
            ".authinfo.gpg",
            ".wgetrc",
            ".curlrc",
            "_curlrc",
            "_netrc",
            ".hgrc",
            ".boto",
            ".minikube",
            "adbkey",
        ];
        for raw in blocked_components {
            let path = std::env::temp_dir().join("home").join(raw).join("x");
            assert!(
                check_sensitive_components(&path).is_err(),
                "{raw} 条目被删除时此断言必须变红"
            );
        }
        let gcloud_adc = std::env::temp_dir()
            .join("home")
            .join(".config")
            .join("gcloud")
            .join("application_default_credentials.json");
        assert!(
            check_sensitive_components(&gcloud_adc).is_err(),
            "gcloud ADC(双层目录名命中)必须被拦"
        );
        for prefix in [
            "/root/",
            "/proc/",
            "/sys/",
            "/etc/security/opasswd",
            // Round-14 复审 minor:旗舰精确条目本身此前无字面删除钉
            // (其余 sudoers 断言全走 `sudoers.*` 通配),清单行被"清理"
            // 时全绿;连同其余五个无钉成员一并逐字面钉住。
            "/etc/sudoers",
            "/etc/gshadow",
            "/var/log/auth",
            "/etc/shadow~",
            "/var/root/",
        ] {
            let path = Path::new(prefix).join("probe");
            assert!(
                check_sensitive_components(&path).is_err(),
                "{prefix} 前缀条目被删除时此断言必须变红"
            );
        }
    }

    /// Round-11 复审 minor:目录作用域尾序列(gh/Maven/sops/Chrome 的
    /// 通用命名凭据文件)——清单成员在位命中、无辜同名文件在别处放行、
    /// 大小写变体跟随卷语义(与组件清单同判据)。清单被删条目即红。
    #[test]
    fn directory_scoped_credential_tails_follow_platform_case_semantics() {
        let blocked = [
            &[".config", "gh", "hosts.yml"][..],
            &[".m2", "settings.xml"][..],
            &[".config", "sops", "age", "keys.txt"][..],
            &[".config", "google-chrome", "Local State"][..],
            &[".config", "chromium", "Local State"][..],
            // Round-14 复审:XDG git 凭据/配置(gradle/containers/android
            // 批同入)——硬编码字面量,清单条目被删除时本钉变红。
            &[".config", "git", "credentials"][..],
            &[".config", "git", "config"][..],
            &[".gradle", "gradle.properties"][..],
            &[".config", "containers", "auth.json"][..],
            &[".android", "debug.keystore"][..],
        ];
        let home = std::env::temp_dir();
        for tail in blocked {
            let mut path = home.clone();
            path.push("repo");
            for component in tail {
                path.push(component);
            }
            assert!(
                check_sensitive_components(&path).is_err(),
                "{} must be refused",
                path.display()
            );
            // 无辜同名文件:不在凭据目录序列之下(父目录不同/深度不足)。
            let mut innocent = home.clone();
            innocent.push("repo");
            innocent.push(tail[tail.len() - 1]);
            assert!(
                check_sensitive_components(&innocent).is_ok(),
                "{} at an arbitrary directory is an ordinary file",
                innocent.display()
            );
            let mut sibling = home.clone();
            sibling.push("repo");
            sibling.push("docs");
            sibling.push(tail[tail.len() - 1]);
            assert!(
                check_sensitive_components(&sibling).is_ok(),
                "{} under an unrelated directory is an ordinary file",
                sibling.display()
            );
            // 大小写变体跟随卷语义(macOS/Windows 折叠命中,Linux 不同文件)。
            let mut variant = home.clone();
            variant.push("repo");
            for component in tail.iter().take(tail.len() - 1) {
                variant.push(component.to_uppercase());
            }
            variant.push(tail[tail.len() - 1].to_uppercase());
            if cfg!(windows) || cfg!(target_os = "macos") {
                assert!(
                    check_sensitive_components(&variant).is_err(),
                    "case-insensitive volumes must fold: {}",
                    variant.display()
                );
            } else {
                assert!(
                    check_sensitive_components(&variant).is_ok(),
                    "Linux stays byte-exact: {}",
                    variant.display()
                );
            }
        }
        // 逐常量循环驱动(删除任一条目即红):清单成员恰好末 N 分量命中。
        for tail in BLOCKED_PATH_TAILS {
            let mut path = home.clone();
            path.push("anywhere");
            for component in tail.iter() {
                path.push(component);
            }
            assert!(
                check_sensitive_components(&path).is_err(),
                "tail {tail:?} must be refused"
            );
        }
    }

    #[test]
    fn default_credential_store_additions_block_across_spellings() {
        // Round-14 复审:同型默认凭据存储补遗(yarn/authinfo/wget/curl/
        // hg/boto/minikube/adb,见清单内注释)的在位命中与无辜名放行;
        // XDG git 尾序列的行为钉在
        // directory_scoped_credential_tails_follow_platform_case_semantics
        // 与 CLI 镜像(artifact 门/会话车道)。
        let blocked = [
            ".yarnrc.yml",
            ".yarnrc",
            ".authinfo",
            ".authinfo.gpg",
            ".wgetrc",
            ".curlrc",
            "_curlrc",
            "_netrc",
            ".hgrc",
            ".boto",
            "adbkey",
        ];
        let home = std::env::temp_dir();
        for raw in blocked {
            let path = home.join("repo").join(raw).join("probe");
            assert!(
                check_sensitive_components(&path).is_err(),
                "{raw} must be refused"
            );
        }
        // `.minikube` 是目录名:目录之下任意文件命中。
        assert!(
            check_sensitive_components(&home.join("repo").join(".minikube").join("ca.key"))
                .is_err()
        );
        // 无辜名:共享前缀/同名不同形不受牵连(adbkey.pub 是公钥,必须可读)。
        for benign in [
            "yarnrc.yml",
            "authinfo2",
            "wgetrc.md",
            "curlrc-notes",
            "adbkey.pub",
            "minikube-dir",
        ] {
            let path = home.join("repo").join(benign).join("probe");
            assert!(
                check_sensitive_components(&path).is_ok(),
                "{benign} is an ordinary name"
            );
        }
    }

    #[test]
    fn env_variant_case_semantics_follow_platform() {
        // 与 uppercase_credential_variants_follow_platform 同判据:.ENV.LOCAL
        // 这类大写变体在大小写不敏感卷(macOS/Windows)上解析到同一文件,
        // 词法前缀判定必须同样折叠;Linux 上是不同文件,不拦。
        let path = std::env::temp_dir().join("app").join(".ENV.LOCAL");
        if cfg!(windows) || cfg!(target_os = "macos") {
            assert!(check_sensitive_components(&path).is_err());
        } else {
            assert!(check_sensitive_components(&path).is_ok());
        }
    }

    #[test]
    fn system_prefixes_follow_component_boundaries() {
        // 系统区前缀按组件序列比较(与 CLI mirror 同一谓语):前缀自身与其
        // 嵌套路径必须被拒;仅共享前缀字节的同名文件是普通可读文件,必须
        // 放行——字节 starts_with 会误拒 `/etc/shadowed` 一类路径。
        // 末分量带 `*` 的开放家族条目(轮转日志)单独驱动,`{prefix}probe`
        // 的放行断言不适用(它自身就是前缀命中)。
        for prefix in BLOCKED_PREFIXES
            .iter()
            .copied()
            .filter(|prefix| !prefix.ends_with('*'))
        {
            let base = Path::new(prefix);
            assert!(check_sensitive_components(base).is_err(), "{prefix}");
            assert!(
                check_sensitive_components(&base.join("probe")).is_err(),
                "{prefix}/probe must be refused"
            );
            if !prefix.ends_with('/') {
                assert!(
                    check_sensitive_components(&PathBuf::from(format!("{prefix}probe"))).is_ok(),
                    "{prefix}probe is a different file and must stay readable"
                );
            }
        }
        // Round-9 复审 MAJOR:组件化改写曾把字节前缀时代的真实家族成员静
        // 默放行——精确枚举的变体逐一拒绝,轮转家族走末分量组件前缀,
        // 无辜同名保持放行。逐项驱动消费方判定(check_sensitive_components
        // → canonical_system_prefix_hit),清单改写若再收窄覆盖即红。
        for variant in ["/etc/shadow-", "/etc/gshadow-", "/var/log/auth.log"] {
            assert!(
                check_sensitive_components(Path::new(variant)).is_err(),
                "{variant} must be refused"
            );
        }
        // Round-10 复审 MAJOR:备份家族——管理员 `cp` 的 `.bak`/`.old`
        // (默认 umask 下常是全局可读,比原件更暴露)、pacman 的
        // `.pacnew`、vipw 的 `.lock`、编辑器的 `~`,在字节前缀时代同样
        // 被拦,组件化改写曾一并放行。开放后缀走 `shadow.*` 家族条目,
        // `~` 走精确条目,无辜同名不回捕。
        for variant in [
            "/etc/shadow.bak",
            "/etc/shadow.old",
            "/etc/shadow.lock",
            "/etc/gshadow.bak",
            "/etc/gshadow~",
            "/etc/sudoers.bak",
            "/etc/sudoers.old",
            "/etc/sudoers.save",
            "/etc/sudoers.orig",
            "/etc/sudoers.pacnew",
            "/etc/sudoers.tmp",
            "/etc/sudoers~",
        ] {
            assert!(
                check_sensitive_components(Path::new(variant)).is_err(),
                "{variant} must be refused by the backup-family prefix"
            );
        }
        assert!(
            check_sensitive_components(Path::new("/etc/sudoers.d/01-admin")).is_err(),
            "sudoers.d drop-ins must be refused"
        );
        for rotated in [
            "/var/log/auth.log.1",
            "/var/log/auth.log.2.gz",
            "/var/log/auth.log.10",
        ] {
            assert!(
                check_sensitive_components(Path::new(rotated)).is_err(),
                "{rotated} must be refused by the rotation-family prefix"
            );
        }
        for benign in [
            "/etc/shadowed",
            "/etc/shadowprobe",
            "/etc/sudoersedit.md",
            "/var/log/authprobe",
            "/var/log/autolog.txt",
        ] {
            assert!(
                check_sensitive_components(Path::new(benign)).is_ok(),
                "{benign} is not a system-sensitive file and must stay readable"
            );
        }
        // 大小写变体跟随平台折叠语义(纯 ASCII 比较在字节精确平台上不命中)。
        let folded = cfg!(windows) || cfg!(target_os = "macos");
        assert_eq!(
            check_sensitive_components(Path::new("/Etc/Sudoers")).is_err(),
            folded,
            "case variants follow the platform's volume folding"
        );
    }

    /// Round-5 复审 M5:macOS 的 canonicalize 把 `/etc/...` 解析成
    /// `/private/etc/...`(/etc 是符号链接),系统前缀黑名单若只看改写后
    /// 的路径就整体失效。别名展开用显式平台参数表达,单元测试在所有平台
    /// 直接驱动,展开被删即红;组件边界语义穿越别名后必须保持
    /// (`/private/etc/shadowed` 不是凭据文件)。
    #[test]
    fn system_prefixes_survive_the_macos_private_alias() {
        assert!(
            canonical_system_prefix_hit(Path::new("/private/etc/shadow"), true),
            "the aliased /etc/shadow spelling must hit on an alias-expanding platform"
        );
        assert!(
            canonical_system_prefix_hit(Path::new("/private/etc/ssh/authorized_keys"), true),
            "nested paths under the aliased prefix must hit"
        );
        assert!(
            !canonical_system_prefix_hit(Path::new("/private/etc/shadowed"), true),
            "the component boundary must survive alias expansion"
        );
        assert!(
            !canonical_system_prefix_hit(Path::new("/private/etc/shadowprobe"), true),
            "same-name files under the aliased prefix must stay readable"
        );
        assert!(
            !canonical_system_prefix_hit(Path::new("/private/etc/shadow"), false),
            "without alias expansion (non-macOS platforms) the raw spelling is not a hit"
        );
        assert!(
            canonical_system_prefix_hit(Path::new("/etc/shadow"), false),
            "the direct spelling must keep matching on every platform"
        );
        assert!(
            !canonical_system_prefix_hit(Path::new("/private/etc/not-blocked"), true),
            "unrelated /private/etc files stay readable"
        );
        assert!(
            !canonical_system_prefix_hit(Path::new("/private/usr/local/bin/tool"), true),
            "only the symlinked system roots are aliased"
        );
        // Round-10 复审 minor:别名展开覆盖 etc/var/tmp 三根,轮转家族
        // 在 macOS 上以 `/private/var/log/...` 拼写出现,`/private/var`
        // 腿一旦收窄,已有 only-etc 断言仍然全绿——此处钉住 var 腿的
        // 命中与无辜放行两个方向。
        assert!(
            canonical_system_prefix_hit(Path::new("/private/var/log/auth.log.1"), true),
            "the rotated auth log under the aliased /var must hit"
        );
        assert!(
            !canonical_system_prefix_hit(Path::new("/private/var/log/authprobe"), true),
            "the component boundary must survive the aliased /var leg"
        );
    }

    /// 清单自律:`*` 只允许出现在末分量且剥掉星号后非空——裸 `*` 会退化
    /// 成逐路径全匹配,内嵌 `*` 只会按字面比较而静默失效。清单改写无法
    /// 靠编译器约束,测试运行时逐条钉住(Round-10 复审 minor)。
    #[test]
    fn wildcard_entries_keep_list_discipline() {
        for prefix in BLOCKED_PREFIXES {
            let components: Vec<&str> = prefix.split('/').filter(|c| !c.is_empty()).collect();
            for (index, component) in components.iter().enumerate() {
                if component.ends_with('*') {
                    assert!(
                        index + 1 == components.len(),
                        "{prefix}: a wildcard component must be the last one"
                    );
                    assert!(
                        component.len() > 1,
                        "{prefix}: a bare '*' component would match every path"
                    );
                }
            }
        }
        // 家族条目自身端到端可命中(星号剥离后按组件前缀比较)。
        assert!(canonical_system_prefix_hit(
            Path::new("/etc/sudoers.bak"),
            false
        ));
        assert!(canonical_system_prefix_hit(
            Path::new("/etc/shadow.lock"),
            false
        ));
    }

    /// Round-5 复审:canonicalize 失败的回退分支必须拒绝含 `..` 的未规范
    /// 化拼写——组件有界比对会把 ParentDir 当普通不匹配组件。已存在路径
    /// 走 canonical 成功分支,不受影响。
    #[test]
    fn unresolvable_dotdot_paths_fail_closed() {
        let nested = std::env::temp_dir().join("pinvou3-path-policy-missing/../etc-shadow-probe");
        assert!(validate_user_path(&nested.to_string_lossy()).is_err());
        // Round-6 复审 M3:断言对象必须是规范化后的拼写——macOS 的
        // TMPDIR(/var/folders/...)经 canonicalize 解析到 /private/var/...,
        // 与未展开的 temp_dir() 拼写不等,原断言在 macOS 上必红。
        let existing = std::env::temp_dir();
        let canonical_existing =
            std::fs::canonicalize(&existing).unwrap_or_else(|_| existing.clone());
        let ok =
            validate_user_path(&existing.to_string_lossy()).expect("existing path stays valid");
        assert_eq!(ok, canonical_existing);
    }

    /// 引擎清单单源锚定（const 断言的运行期镜像 + 残差登记）：本闸两份黑名单
    /// 是引擎 `sensitive_paths` 三份清单的 Pinvou 子集，引擎侧任何改名/删除
    /// 同时被编译期断言与本用例拦截；残差（引擎有、Pinvou 不登记）按注释
    /// 登记的集合逐项钉死，改成任何一侧都要有意识。引擎四个面各有覆盖方向
    /// 循环：引擎新增条目必须被采纳或登记为残差，不再能静默漏过。
    #[test]
    fn forkguard_blocked_lists_are_the_anchored_subset_of_engine_inventory() {
        use sensitive_paths::{
            SENSITIVE_ABSOLUTE_PREFIXES, SENSITIVE_COMPONENT_NAMES, SENSITIVE_DIRECTORY_NAMES,
            SENSITIVE_FILE_NAMES,
        };

        let in_engine = |entry: &str| {
            SENSITIVE_DIRECTORY_NAMES.contains(&entry)
                || SENSITIVE_FILE_NAMES.contains(&entry)
                || SENSITIVE_COMPONENT_NAMES.contains(&entry)
        };

        // 子集方向（编译期断言的镜像）：每项都在引擎三份清单内。
        for entry in BLOCKED_COMPONENTS {
            assert!(in_engine(entry), "{entry} 应在引擎清单");
        }
        for entry in BLOCKED_PREFIXES {
            assert!(
                SENSITIVE_ABSOLUTE_PREFIXES.contains(&entry),
                "{entry} 应在引擎前缀清单"
            );
        }

        // 引擎组件面派生的裸 id_*（预登记迁移已落地，const 断言镜像）：保留
        // 在组件黑名单且逐项锚定在引擎组件面上。
        for key in ID_COMPONENT_KEYS {
            assert!(BLOCKED_COMPONENTS.contains(key), "{key} 应保留");
            assert!(
                SENSITIVE_COMPONENT_NAMES.contains(key),
                "{key} 应锚定在引擎组件面"
            );
        }

        // 刻意的引擎残差：目录面（多段/未采纳目录名——多段字面量在组件相等
        // 比较下永不命中）。
        let dir_residual = [
            ".gnupg/private-keys-v1.d",
            ".config/gcloud",
            ".azure",
            ".config/google-chrome",
            ".mozilla/firefox",
            ".dws",
            ".tmeet",
        ];
        // 文件面残差：`.ssh/*` 密钥/信任文件以路径拼写登记（`id_*` 组件已改
        // 由引擎组件面经 `ID_COMPONENT_KEYS` 派生，目录类由目录面覆盖）；
        // 仅 home 根敏感的裸文件名登记会误拦同名工作
        // 目录；`credentials.json`/`.env` 是引擎注释认可的任意组件敏感面，
        // Pinvou 已登记。
        let file_residual = [
            ".ssh/id_rsa",
            ".ssh/id_ed25519",
            ".ssh/id_ecdsa",
            ".ssh/id_dsa",
            ".ssh/authorized_keys",
            ".ssh/config",
            ".kube/config",
            ".docker/config.json",
            ".aws/config",
            ".aws/credentials",
            ".config/gcloud/application_default_credentials.json",
            ".config/gcloud/credentials.db",
            ".azure/msal_token_cache.json",
            ".azure/accessTokens.json",
            ".config/google-chrome/Default/Cookies",
            ".config/google-chrome/Default/Login Data",
            ".config/google-chrome/Local State",
            ".gnupg/secring.gpg",
            "credentials",
            "secrets",
            ".pgp",
            ".gpg",
            ".netrc",
            ".git-credentials",
        ];
        for residual in dir_residual.iter().chain(file_residual.iter()) {
            assert!(
                !BLOCKED_COMPONENTS.contains(&residual),
                "{residual} 若要登记进组件黑名单，须同步更新本残差登记与 const 断言"
            );
            assert!(
                in_engine(residual),
                "残差登记本身必须仍在引擎清单内: {residual}"
            );
        }

        // 组件面残差：`.dws`/`.tmeet` 引擎按「任意组件」登记（组件面清单），
        // 本闸不采纳——摄取前缀面无此二者、目录面残差同名登记（命令通道语料
        // 经 SENSITIVE_DIR_NAMES 别名承担）。引擎组件面若新增条目，覆盖循环
        // 会强制在此登记或采纳。
        let component_residual = [".dws", ".tmeet"];
        for residual in component_residual {
            assert!(
                !BLOCKED_COMPONENTS.contains(&residual),
                "{residual} 若要登记进组件黑名单，须同步更新本残差登记"
            );
            assert!(
                SENSITIVE_COMPONENT_NAMES.contains(&residual),
                "残差登记本身必须仍在引擎组件面内: {residual}"
            );
        }

        // 前缀面残差：备份/`-` 变体、sudoers.d 与轮变体已被 starts_with 语义
        // 覆盖（`/var/log/auth.log` 为 #87 评审轮新增，被采纳的
        // `/var/log/auth` 前缀覆盖）。
        for residual in [
            "/etc/shadow-",
            "/etc/shadow.bak",
            "/etc/gshadow-",
            "/etc/sudoers-",
            "/etc/sudoers.bak",
            "/etc/sudoers.d/",
            "/var/log/auth.log",
        ] {
            assert!(
                !BLOCKED_PREFIXES.contains(&residual),
                "{residual} 若要登记，须同步更新本残差登记"
            );
            assert!(SENSITIVE_ABSOLUTE_PREFIXES.contains(&residual));
        }

        // 引擎覆盖方向：每个引擎条目必须被采纳或登记为残差。仅靠子集断言时，
        // 引擎新增条目会静默漏过（正是单源化要暴露的漂移类）；本方向强制引
        // 擎新增在本表显式登记（或采纳），四个面各一。
        for engine_dir in SENSITIVE_DIRECTORY_NAMES {
            assert!(
                BLOCKED_COMPONENTS.contains(engine_dir) || dir_residual.contains(engine_dir),
                "引擎目录 {engine_dir} 未被采纳也不在残差登记中"
            );
        }
        for engine_file in SENSITIVE_FILE_NAMES {
            assert!(
                BLOCKED_COMPONENTS.contains(engine_file) || file_residual.contains(engine_file),
                "引擎文件 {engine_file} 未被采纳也不在残差登记中"
            );
        }
        for engine_component in SENSITIVE_COMPONENT_NAMES {
            assert!(
                BLOCKED_COMPONENTS.contains(engine_component)
                    || component_residual.contains(engine_component),
                "引擎组件 {engine_component} 未被采纳也不在残差登记中"
            );
        }
        for engine_prefix in SENSITIVE_ABSOLUTE_PREFIXES {
            let subsumed = BLOCKED_PREFIXES
                .iter()
                .any(|p| engine_prefix.starts_with(*p));
            assert!(
                BLOCKED_PREFIXES.contains(engine_prefix) || subsumed,
                "引擎前缀 {engine_prefix} 未被采纳也不被 starts_with 覆盖"
            );
        }
    }
}
