//! 通用 CLI 连接器管道 —— 抽自 `feishu.rs`,供飞书 / 企微等"官方 CLI 连接器"共享。
//!
//! 设计(开发方案 C):公共的"起子进程 / 抑黑窗 / 抓授权 URL / 出二维码 / 收发事件 /
//! 取消"逻辑收口在此;各连接器(`feishu.rs` / `wecom.rs`)只持有自己的 [`CliCtx`]
//! 薄声明 + 一段连接编排函数,调本模块的公共件。
//!
//! **drain 多态性说明**:[`drain_for_url`] 只发 URL(`String`),供飞书/企微共享。
//! tmeet/dingtalk 有各自的私有 drain(`drain_for_auth_url`/`drain_for_auth_event`),
//! 因为它们在同一管道里额外抓取安全日志行 / user_code,channel 元素类型分别为
//! `(Option<String>, Option<String>)` 和 `AuthEvent` enum。这是真实业务差异,
//! 强行泛型化会增加闭包复杂度而收益有限——三者的 `BufReader::lines` 循环骨架
//! 虽同构,但行处理逻辑不可统一。
//!
//! 连接状态(长驻子进程 PID + 取消标志)用一个 [`ConnectorConn`] 按连接器 id 复用,
//! `lib.rs` 里 `.manage(ConnectorConn::default())` 注册一次,飞书 / 企微共用。

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter};

use crate::features::connectors::skill_gate;
use crate::platform::process::{REAP_GRACE, Reap, reap_killed_child};

/// 一个 CLI 连接器的运行上下文(薄声明的"可执行"部分)。
/// 全是 `'static` 引用,故可 `Copy`——能直接搬进抓 URL 的后台线程。
#[derive(Clone, Copy)]
pub struct CliCtx {
    /// 逻辑 CLI 名,如 `"lark-cli"` / `"wecom-cli"`。
    /// 平台层负责解析到实际可执行文件或 npm 全局 shim。
    pub cli_bin: &'static str,
    /// 进程环境(代理绕行等):飞书 `LARK_CLI_NO_PROXY=1`。
    pub envs: &'static [(&'static str, &'static str)],
    /// 从子进程输出里抓授权 URL 的域名白名单(命中其一才算本连接器的 URL)。
    pub auth_domains: &'static [&'static str],
}

impl CliCtx {
    /// 构造子进程命令:平台层负责可执行文件解析和运行时 PATH,连接器层只注入 envs。
    /// `program` 可以是 `cli_bin` 本身,也可以是 `"npm"` / `"npx"` 等。
    pub fn base_cmd(&self, program: &str) -> Command {
        let mut c = connector_cli_command(self.cli_bin, program);
        for (k, v) in self.envs {
            c.env(k, v);
        }
        c
    }

    /// `<cli_bin> <args>`(抑黑窗 + envs)。
    pub fn cli(&self, args: &[&str]) -> Command {
        let mut c = self.base_cmd(self.cli_bin);
        c.args(args);
        c
    }

    /// 从一行输出里抓本连接器的授权 URL(`https://` 开头且命中 `auth_domains`)。
    pub fn extract_url(&self, line: &str) -> Option<String> {
        let i = line.find("https://")?;
        let url: String = line[i..]
            .chars()
            .take_while(|c| !c.is_whitespace())
            .collect();
        if self.auth_domains.iter().any(|d| url.contains(d)) {
            Some(url)
        } else {
            None
        }
    }
}

// ─────────────────────────── 子进程构造 ───────────────────────────

fn connector_cli_command(cli_bin: &str, program: &str) -> Command {
    crate::platform::os::connector_cli_command(cli_bin, program)
}

pub fn apply_user_npm_prefix(cmd: &mut Command) {
    crate::platform::os::apply_user_npm_prefix(cmd);
}

// ─────────────────────────────── 公共执行件 ───────────────────────────────

/// 授权 CLI 输出行的安全化（打日志 / 经事件通道上屏前统一过一道）：空行丢弃；
/// 命中敏感词整行替换为占位符；其余行截断到 320 字符。敏感词为
/// `access_token` / `refresh_token` / `authorization:` / `bearer `，`redact_bare_token`
/// 再追加兜底子串 `token`——tmeet 的输出会出现不带下划线/驼峰的 token 字样需要兜底；
/// dingtalk 不开兜底（其正常 JSON 行含 camelCase token 字段名，开了会把非敏感行
/// 也整体吞掉，属行为变化）。原先 tmeet/dingtalk 各有一份本地副本，已收编至此。
pub(crate) fn safe_auth_log_line(line: &str, redact_bare_token: bool) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    let mut sensitive = lower.contains("access_token")
        || lower.contains("refresh_token")
        || lower.contains("authorization:")
        || lower.contains("bearer ");
    if redact_bare_token {
        sensitive = sensitive || lower.contains("token");
    }
    if sensitive {
        return Some("[redacted credential line]".to_string());
    }
    Some(trimmed.chars().take(320).collect())
}

/// 探测失败必须按原因分型(见各变体)——「没装」与「装了但挂死」混为一谈,
/// 会误导用户重装或谎报已断开。注意各 `*_cli_present` 仍把 Err 折叠成
/// false,ensure 流程因此仍可能对挂死 CLI 重跑安装(布尔折叠的已知残余);
/// 这里的分型保证的是人类可读诊断与调用方降级分支都不再误导。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProbeError {
    /// 进程没起来(≈CLI 二进制不存在,「未安装」),可安全按未安装降级。
    Spawn(String),
    /// 进程已启动但超时被杀(可能挂在网络/代理),重试有意义。
    Timeout(String),
    /// 进程已启动但 wait/管道失败——**不是**「未安装」,按安装缺失提示是错的。
    Other(String),
}

impl ProbeError {
    /// 在**包装人类可读文案之前**对 platform::process 的原始错误分型。
    /// 分型依据是本仓库自家模块的报错格式(`spawn {program} failed: ` 出自
    /// platform/process.rs 的 spawn 失败路径,` timed out after ` 出自超时
    /// 路径),由下方测试钉住。分型必须吃原始错误——先包装后分型会让前缀
    /// 匹配永远落空,降级分支随之失效(本轮修掉的回归)。
    fn classify(raw: String) -> Self {
        if raw.starts_with("spawn ") {
            Self::Spawn(raw)
        } else if raw.contains(" timed out after ") {
            Self::Timeout(raw)
        } else {
            Self::Other(raw)
        }
    }

    /// 人类可读的探测失败文案;分类与 [`ProbeError`] 严格对应。
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Spawn(error) => {
                format!("启动失败: {error}(需要先完成对应连接器 CLI 的在线安装)")
            }
            Self::Timeout(error) => {
                format!("{error}(CLI 探测超时;可能被网络/代理卡住,请重试)")
            }
            Self::Other(error) => {
                format!("{error}(CLI 探测执行异常;进程已启动但通信失败,请重试)")
            }
        }
    }
}

/// 跑探测命令并保留**分型后的错误**,供断开登录路径按类降级。
///
/// 在 `spawn_blocking` 里调。带 30s 兜底超时(kill-tree):这些调用绝大多数
/// 是 `--version` / `auth status` / `auth logout` 一类的短命令,但 npm-shim
/// CLI 曾实测会卡在网络/代理/无 TTY 提示上无限挂起(同 `run_with_timeout`
/// 的注释)。不设上限会让 connector 状态查询与首帧 auth-gate 刷新永久转圈;
/// 超时按失败处理,调用方已有各自的降级分支。用 kill-tree 变体:超时只杀
/// wrapper 会把 node 孙进程连同管道一起留下,飞书 `auth login --device-code`
/// 轮询里的阻塞调用会反复超时、反复孤儿化进程。
///
/// `Spawn` ≈ 真未安装(保留「未安装」降级),`Timeout`/`Other` 都发生在
/// 进程**已经启动**之后,按「未安装」降级是错的——断开登录路径若把探测
/// 超时当未安装,会在 `auth logout` 根本没执行、token 未撤销的情况下向
/// 用户谎报「已断开」。
pub(crate) fn run_probe(cmd: Command) -> Result<(bool, String, String), ProbeError> {
    const CLI_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
    crate::platform::process::output_with_timeout_and_kill_tree(cmd, CLI_PROBE_TIMEOUT)
        .map_err(ProbeError::classify)
        .map(|out| {
            (
                out.status.success(),
                String::from_utf8_lossy(&out.stdout).into_owned(),
                String::from_utf8_lossy(&out.stderr).into_owned(),
            )
        })
}

/// 断开登录路径对探测结果的**唯一裁决**:只有「进程没起来」
/// ([`ProbeError::Spawn`] ≈ 二进制不存在)允许按未安装降级。
///
/// 其余一切非成功探测——超时、执行异常、乃至探测**成功执行**但 CLI
/// 自报失败(`--version` 退出非零/版本无法解析,各连接器折叠为
/// `Ok(false)` 传入)——都只说明 CLI 没能正常响应,不能证明它不存在:
/// 此时 `auth logout` 根本没执行、token 未撤销,按未安装降级会清掉
/// bundle store 并向用户谎报「已断开」。该分支历史上曾被调用点
/// 折叠成「未安装」,故裁决收敛到本函数并用测试钉死。
#[derive(Debug)]
pub(crate) enum LogoutProbeVerdict {
    /// 探测通过,继续执行 `auth logout`。
    Installed,
    /// 真未安装,调用方按 `installed:false` 降级(清 bundle store 合法)。
    NotInstalled,
    /// 凭据状态未确认,携带人类可读文案原样上抛,不得动 bundle store。
    Unconfirmed(String),
}

/// 按 [`LogoutProbeVerdict`] 裁决;`label` 是连接器中文名(如「钉钉」),
/// 用于「已安装但版本探测未通过」文案。
pub(crate) fn logout_probe_verdict(
    label: &str,
    probe: Result<bool, ProbeError>,
) -> LogoutProbeVerdict {
    match probe {
        Ok(true) => LogoutProbeVerdict::Installed,
        Ok(false) => LogoutProbeVerdict::Unconfirmed(format!(
            "{label} CLI 已安装但版本探测未通过，登录状态未确认；请重试或重新安装 CLI 后再断开"
        )),
        Err(ProbeError::Spawn(_)) => LogoutProbeVerdict::NotInstalled,
        Err(error) => LogoutProbeVerdict::Unconfirmed(error.message()),
    }
}

/// 同 [`run_probe`],但把分型错误折叠成人类可读文案,供不区分失败原因的
/// 调用方(状态轮询、ensure 流程、`auth logout` 执行本身)继续用 `?` 上抛。
pub fn run(cmd: Command) -> Result<(bool, String, String), String> {
    run_probe(cmd).map_err(|error| error.message())
}

/// 跑命令并带**超时**(防 npm/npx 卡在网络 / 代理 / 无 TTY 提示上无限转)。
///
/// 两处关键(修 "install 卡死 / 无从诊断",飞书 / 企微 / 以后所有连接器共用):
/// 1. **stdin 显式接 null**。app 是无窗口 GUI 进程,继承来的 stdin 是坏句柄,
///    CLI 安装器(`@wecom/cli` / `@larksuite/cli` 等)读它会**死等 → 每次卡到超时**
///    (终端手动跑却几十秒就成)。给个立即 EOF 的 null stdin,安装器走非交互分支跑通。
/// 2. **stdout/stderr 落日志文件**(不再 `null` 丢弃),失败可诊断:
///    `~/.pinvou3/cli-install.log`。写文件不是管道、无写满死锁之虞。
pub fn run_with_timeout(mut cmd: Command, secs: u64) -> Result<bool, String> {
    let log_path = crate::platform::paths::pinvou3_home().join("cli-install.log");
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let (out, err) = match std::fs::File::create(&log_path) {
        Ok(f) => match f.try_clone() {
            Ok(f2) => (Stdio::from(f), Stdio::from(f2)),
            Err(_) => (Stdio::null(), Stdio::null()),
        },
        Err(_) => (Stdio::null(), Stdio::null()), // 落不了盘也别卡,回退丢弃
    };
    cmd.stdin(Stdio::null()).stdout(out).stderr(err);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("启动在线安装失败: {e}(请检查应用运行时是否完整)"))?;
    let start = Instant::now();
    loop {
        match child.try_wait().map_err(|e| format!("wait: {e}"))? {
            Some(status) => return Ok(status.success()),
            None => {
                if start.elapsed() > Duration::from_secs(secs) {
                    // Termination must be confirmed before the bounded
                    // reap: if kill() fails the installer may still be
                    // alive, and waiting on it could outlive the timeout
                    // budget, so report the failure immediately instead.
                    if let Err(e) = child.kill() {
                        return Err(format!(
                            "CLI 安装超时({secs}s):可能是网络 / 代理(Clash)拦截;终止安装进程失败({e})(日志见 {})",
                            log_path.display()
                        ));
                    }
                    // Reap the killed child on a bounded budget: without
                    // wait() it lingers as a zombie and repeated timeouts
                    // exhaust kernel process-table entries
                    // (Pinvou/pinvou3#1097).
                    let reap_note = match reap_killed_child(&mut child, REAP_GRACE) {
                        Reap::Reaped => String::new(),
                        Reap::Abandoned => {
                            format!(";终止请求已发出但 {:?} 内仍未退出,进程可能驻留", REAP_GRACE)
                        }
                        Reap::Failed(e) => format!(";回收安装进程失败({e})"),
                    };
                    return Err(format!(
                        "CLI 安装超时({secs}s):可能是网络 / 代理(Clash)拦截{reap_note}(日志见 {})",
                        log_path.display()
                    ));
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    }
}

/// Bounded reap of a killed connector child with the shared grace budget,
/// so auth-timeout and cancel paths do not leave zombies behind. The
/// outcome is deliberately discarded by callers: their user-facing
/// messages describe the auth result, not process hygiene.
pub(crate) fn reap_after_kill(child: &mut Child) {
    let _ = reap_killed_child(child, REAP_GRACE);
}

/// 标准 base64 编码(避免引新依赖)。
pub fn b64(data: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(A[((n >> 18) & 63) as usize] as char);
        out.push(A[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            A[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            A[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// 从一段输出里抓第一个 JSON 对象(CLI `--json` 有时夹带 spinner / 提示行)。
pub fn parse_json(s: &str) -> Option<Value> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end < start {
        return None;
    }
    serde_json::from_str(&s[start..=end]).ok()
}

/// 解析 CLI `--version` 输出为三段语义版本:取字符串中首个连续数字段序列,
/// 不足三段补 0(假想的两段式「2.0」→(2,0,0),避免误判未装而触发降级重装),
/// 一个数字段都没有则 `None`。wecom/tmeet 的最低版本门共用;
/// 输出里程序名等位置可能带数字的,调用方先自行切片再传入(tmeet 见 marker 定位)。
pub fn parse_semver3(s: &str) -> Option<(u64, u64, u64)> {
    let mut nums = s
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.parse::<u64>().ok());
    let major = nums.next()?;
    Some((major, nums.next().unwrap_or(0), nums.next().unwrap_or(0)))
}

/// 把授权 URL 渲染成二维码(SVG data URL,供前端 `<img src>` 直接显示)。
/// 纯 Rust(qrcode crate),不依赖具体 CLI 的 qrcode 子命令——各连接器通用。
/// 失败返回 `None`(前端回退开浏览器)。
pub fn make_qr(url: &str) -> Option<String> {
    use qrcode::QrCode;
    use qrcode::render::svg;
    let code = QrCode::new(url.as_bytes()).ok()?;
    let svg_xml = code
        .render::<svg::Color<'_>>()
        .min_dimensions(220, 220)
        .quiet_zone(true)
        .build();
    Some(format!(
        "data:image/svg+xml;base64,{}",
        b64(svg_xml.as_bytes())
    ))
}

/// Wraps the QR PNG written to disk by the CLI (wecom-cli `--output-qrcode`) as a
/// data URL so the frontend can show it directly via `<img src>`. Validates the PNG
/// signature plus the IEND tail; non-PNG / truncated files return `None` (the
/// caller falls back to [`make_qr`]).
pub fn png_data_url(bytes: &[u8]) -> Option<String> {
    const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    // The IEND chunk is always length(0) + type + CRC: the CRC over the IEND
    // content is fixed, so the whole tail can be compared verbatim. Checking only
    // the signature would accept a half-written file (header only) and the browser
    // would render a broken image.
    const IEND_TAIL: &[u8] = b"\x00\x00\x00\x00IEND\xae\x42\x60\x82";
    if bytes.len() < PNG_SIGNATURE.len() + IEND_TAIL.len()
        || !bytes.starts_with(PNG_SIGNATURE)
        || !bytes.ends_with(IEND_TAIL)
    {
        return None;
    }
    Some(format!("data:image/png;base64,{}", b64(bytes)))
}

/// 后台线程:逐行排空一个管道(防写满阻塞),抓到首个本连接器 URL 经 channel 送回。
pub fn drain_for_url<R: std::io::Read + Send + 'static>(
    ctx: CliCtx,
    r: R,
    tx: mpsc::Sender<String>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for line in BufReader::new(r).lines() {
            let line = match line {
                Ok(line) => line,
                Err(error) => {
                    // 管道读取错误通常不可恢复；继续迭代可能反复返回 Err 并空转。
                    log::warn!("[{}] 授权输出读取失败，停止排空：{error}", ctx.cli_bin);
                    break;
                }
            };
            if let Some(u) = ctx.extract_url(&line) {
                let _ = tx.send(u);
            }
        }
    })
}

/// tree-kill 一个 PID,连其子进程(.cmd 拉起的 node)一起。
pub fn kill_pid_tree(pid: u32) {
    crate::platform::os::kill_pid_tree(pid);
}

/// 给前端发连接编排事件(`<id>:qr` / `<id>:connected` / `<id>:error`)。
pub fn emit(app: &AppHandle, event: &str, payload: Value) {
    let _ = app.emit(event, payload);
}

/// `*_ensure_cli` 公共脚手架(飞书 / 企微 / 钉钉 / 腾讯会议四份同构块收编):
/// 已装(`present`)则秒返回;否则跑 `install`(在线安装),装完复检 `present`,
/// 仍未就绪报 `unexecutable_error`。各连接器的预检查(版本下限等)经 `present`
/// 闭包传入。
pub(crate) async fn ensure_cli_with<P, I>(
    id: &'static str,
    present: P,
    unexecutable_error: &'static str,
    install: I,
) -> Result<Value, String>
where
    P: Fn() -> bool + Send + 'static,
    I: FnOnce() -> Result<(), String> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let t = Instant::now();
        // 已装则秒返回 —— 不跑慢吞吞、可能卡死的在线安装。
        let present_now = present();
        eprintln!(
            "[{id}] ensure_cli: cli_present={present_now} in {}ms",
            t.elapsed().as_millis()
        );
        if present_now {
            return Ok::<Value, String>(json!({ "ok": true, "already": true }));
        }
        install()?;
        if !present() {
            return Err(unexecutable_error.to_string());
        }
        Ok::<Value, String>(json!({ "ok": true, "already": false }))
    })
    .await
    .map_err(|e| format!("spawn_blocking: {e}"))?
}

// ──────────────────────── 多连接器共享的连接编排状态 ────────────────────────

/// 按连接器 id 存当前长驻子进程 PID + 取消标志。`lib.rs` 注册一次,飞书 / 企微共用。
#[derive(Default)]
pub struct ConnectorConn {
    slots: Mutex<HashMap<&'static str, Slot>>,
}

#[derive(Default)]
struct Slot {
    pid: Option<u32>,
    cancelled: bool,
}

impl ConnectorConn {
    /// 开始一轮连接前清掉该连接器的取消标志。
    pub fn reset(&self, id: &'static str) {
        if let Ok(mut m) = self.slots.lock() {
            m.entry(id).or_default().cancelled = false;
        }
    }

    /// 置取消标志,返回当前长驻 PID(供 tree-kill)。
    pub fn cancel(&self, id: &'static str) -> Option<u32> {
        if let Ok(mut m) = self.slots.lock() {
            let s = m.entry(id).or_default();
            s.cancelled = true;
            return s.pid;
        }
        None
    }

    pub fn is_cancelled(&self, id: &str) -> bool {
        self.slots
            .lock()
            .ok()
            .and_then(|m| m.get(id).map(|s| s.cancelled))
            .unwrap_or(false)
    }

    pub fn set_pid(&self, id: &'static str, pid: Option<u32>) {
        if let Ok(mut m) = self.slots.lock() {
            m.entry(id).or_default().pid = pid;
        }
    }

    /// 退出收割（lib.rs RunEvent::Exit 调）：杀掉所有槽位登记的长驻子进程树。
    /// 这些 CLI 子进程（.cmd 拉起的 node）没有 kill_on_drop 兜底，宿主进程
    /// 退出后若不显式清理会变孤儿进程继续驻留。
    pub fn kill_all_pids(&self) {
        if let Ok(m) = self.slots.lock() {
            for slot in m.values() {
                if let Some(pid) = slot.pid {
                    kill_pid_tree(pid);
                }
            }
        }
    }
}

/// 首屏提交后刷新鉴权门控的耗时回执(前端只读 elapsed_ms 打启动标记)。
#[derive(Debug, Serialize)]
pub struct ConnectorAuthGateRefresh {
    elapsed_ms: u64,
}

/// 首屏提交后刷新飞书 / 企微 / 钉钉 / 腾讯会议鉴权门控。外部 CLI 在 blocking 线程池并行执行，
/// 不占 Tauri setup 主线程；各自只修改互不重叠的技能目录。四连接器的探测 + 刷新
/// 同一形状,按 [`skill_gate::GATES`] 表驱动。
pub async fn refresh_connector_auth_gates() -> Result<ConnectorAuthGateRefresh, String> {
    let started = Instant::now();
    crate::platform::startup::mark("connector_auth_refresh:start");

    let tasks: Vec<_> = skill_gate::GATES
        .iter()
        .map(|gate| {
            let gate: &'static skill_gate::ConnectorGate = *gate;
            tokio::task::spawn_blocking(move || gate.refresh_step())
        })
        .collect();

    let mut visible_marks = Vec::with_capacity(tasks.len());
    for (gate, task) in skill_gate::GATES.iter().zip(tasks) {
        let visible = task
            .await
            .map_err(|e| format!("{}鉴权探测任务失败: {e}", gate.display_name))??;
        visible_marks.push(format!("{}_visible={visible}", gate.id));
    }
    let elapsed_ms = started.elapsed().as_millis() as u64;
    crate::platform::startup::mark_with_detail(
        "rust",
        "connector_auth_refresh:done",
        &format!("elapsed_ms={elapsed_ms} {}", visible_marks.join(" ")),
    );
    Ok(ConnectorAuthGateRefresh { elapsed_ms })
}

// ─────────────── BundleStore 镜像（marketplace-unification Phase 2）───────────────
//
// 过渡期纪律：连接器的授权文件 / 技能目录 / CLI 二进制仍是权威，bundles.json
// 只镜像安装态；镜像写失败不影响主操作，fail loud 到日志。

/// 连接成功（授权就绪）后登记 CLI 包：`source=Builtin`，并清除 `degraded`
/// （重连即修复，§3.2）。既有记录保留首次登记时间与 extra（见
/// `BundleStore::upsert_preserving`）。
pub fn bundle_store_on_connected(id: &str) {
    use crate::features::marketplace::store::{BundleRecord, BundleSource, BundleStore};
    let record = BundleRecord::installed_now(id, BundleSource::Builtin);
    if let Err(e) = BundleStore::new().upsert_preserving(record) {
        log::warn!("[connectors] bundles.json 镜像写入失败（connect {id}）: {e}");
    }
}

/// 断开（删授权）≠ 卸载：**记录保留** —— `installed` 是存储态，授权存在与否是
/// `ready` 派生态、现算且永不进存储（§3.2）。但当前实现断开后 apply_skills 会
/// 删掉 companion 技能目录，包内容不完整，故按 §3.2 的 Degraded（登记在、资源缺）
/// 标记；修复动作 = 重新连接（重解包技能），与预置重装/上传重导入同构。
/// 记录不存在（从未连接成功过）时 mark_degraded 返回 false，天然无操作。
pub fn bundle_store_on_disconnected(id: &str) {
    if let Err(e) = crate::features::marketplace::store::BundleStore::new()
        .mark_degraded(id, "已断开授权：配套技能已随断开移除，重新连接即可恢复")
    {
        log::warn!("[connectors] bundles.json 镜像写入失败（disconnect {id}）: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, Read};

    const TEST_CTX: CliCtx = CliCtx {
        cli_bin: "test-cli",
        envs: &[],
        auth_domains: &["work.weixin.qq.com", "weixin.qq.com"],
    };

    /// extract_url three-branch matrix: whitelisted domain hits truncate at
    /// whitespace (QR-scan URLs often carry `&` query strings that must not be
    /// cut), non-whitelisted domains do not count as this connector's URL,
    /// and no URL yields None.
    #[test]
    fn extract_url_matrix() {
        let cases = [
            (
                "请打开 https://work.weixin.qq.com/x?a=1&b=2 扫码",
                Some("https://work.weixin.qq.com/x?a=1&b=2"),
            ),
            ("https://example.com/foo", None),
            ("纯文本,没有链接", None),
        ];
        for (input, expected) in cases {
            assert_eq!(
                TEST_CTX.extract_url(input),
                expected.map(str::to_string),
                "{input:?}"
            );
        }
    }

    struct ReadErrorThenPanic {
        failed: bool,
    }

    impl Read for ReadErrorThenPanic {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            if self.failed {
                panic!("读取错误后不应继续轮询同一管道");
            }
            self.failed = true;
            Err(Error::other("test read failure"))
        }
    }

    #[test]
    fn drain_for_url_stops_after_read_error() {
        let (tx, _rx) = mpsc::channel();
        let handle = drain_for_url(TEST_CTX, ReadErrorThenPanic { failed: false }, tx);
        assert!(handle.join().is_ok(), "读取错误后应退出排空线程");
    }

    /// A 30s probe timeout must NOT read as "CLI not installed": the ensure
    /// flow branches on the collapsed boolean and would re-download a CLI
    /// that is merely hung, and `auth logout` of a connected user would
    /// claim the CLI is missing. [`ProbeError`] separates the three classes
    /// for the logout paths, and classification must happen on the RAW
    /// platform error — a previous round classified the human-wrapped
    /// message instead, so the Spawn branch was unreachable and the
    /// missing-CLI logout degrade regressed into a hard error.
    #[test]
    fn probe_error_messages_distinguish_timeout_from_missing_install() {
        // fixtures use the real platform::process message formats.
        let timeout = ProbeError::classify(String::from(
            "lark-cli timed out after 30s: subprocess tree termination requested",
        ));
        assert!(matches!(timeout, ProbeError::Timeout(_)));
        assert!(timeout.message().contains("探测超时"));
        assert!(!timeout.message().contains("在线安装"));

        let spawn = ProbeError::classify(String::from("spawn lark-cli failed: program not found"));
        assert!(matches!(spawn, ProbeError::Spawn(_)));
        assert!(spawn.message().contains("启动失败"));
        assert!(spawn.message().contains("在线安装"));

        // 进程已启动后的 wait/管道失败既非「未安装」也非超时,文案不得再
        // 宣称需要安装(上一轮实现会把这一类误标成「启动失败需安装」)。
        let other = ProbeError::classify(String::from("lark-cli wait error: no child process"));
        assert!(matches!(other, ProbeError::Other(_)));
        assert!(!other.message().contains("在线安装"));

        // 组合回归:run_probe 是 logout 路径的入口,必须端到端产出 Spawn 分型。
        assert!(matches!(
            run_probe(Command::new("pinvou3-no-such-connector-cli-for-tests")),
            Err(ProbeError::Spawn(_))
        ));
    }

    /// 断开登录的裁决表:只有 Spawn(≈二进制不存在)允许按「未安装」
    /// 降级;其余一切非成功探测——超时、执行异常、乃至探测成功执行但
    /// CLI 自报失败(`--version` 退出非零/版本无法解析,调用点折叠为
    /// `Ok(false)` 传入)——都只说明 CLI 没能正常响应,不能证明不存在:
    /// 此时 `auth logout` 根本没执行,按未安装降级会在 token 未撤销时
    /// 清掉 bundle store 并谎报「已断开」。该折叠分支曾真实存在
    /// (dingtalk `Ok(false)` / tmeet `Ok(None)`),故整表钉死。
    #[test]
    fn logout_probe_verdict_degrades_only_on_spawn() {
        let label = "测试";

        // 真未安装:唯一保留降级的路径。
        let spawn = ProbeError::classify(String::from("spawn dws failed: program not found"));
        assert!(matches!(
            logout_probe_verdict(label, Err(spawn)),
            LogoutProbeVerdict::NotInstalled
        ));

        // 探测通过:继续执行 auth logout。
        assert!(matches!(
            logout_probe_verdict(label, Ok(true)),
            LogoutProbeVerdict::Installed
        ));

        // 已安装但版本探测未通过:不得降级,文案须标明「未确认」且不得
        // 诱导重装路径之外的误判。
        match logout_probe_verdict(label, Ok(false)) {
            LogoutProbeVerdict::Unconfirmed(message) => {
                assert!(message.contains(label), "{message}");
                assert!(message.contains("登录状态未确认"), "{message}");
                assert!(!message.contains("未安装"), "{message}");
            }
            other => panic!("Ok(false) 不得按未安装降级,实际 {other:?}"),
        }

        // 超时/执行异常维持原分型文案,同样不得降级。
        let timeout = ProbeError::classify(String::from(
            "dws timed out after 30s: subprocess tree termination requested",
        ));
        match logout_probe_verdict(label, Err(timeout)) {
            LogoutProbeVerdict::Unconfirmed(message) => {
                assert!(message.contains("探测超时"), "{message}");
            }
            other => panic!("Timeout 应转为未确认错误,实际 {other:?}"),
        }
        let other = ProbeError::classify(String::from("dws wait error: no child process"));
        assert!(matches!(
            logout_probe_verdict(label, Err(other)),
            LogoutProbeVerdict::Unconfirmed(_)
        ));
    }

    /// 本地生成二维码:任意 URL 都能出码(不依赖各 CLI 的 qrcode 子命令),
    /// 且是可直接塞进 <img> 的 SVG data URL。
    #[test]
    fn make_qr_local_produces_svg_data_url() {
        let url = "https://accounts.feishu.cn/oauth/authorize?client_id=cli_abc&device_code=xyz&scope=a%20b";
        let qr = make_qr(url).expect("本地二维码生成不应失败");
        assert!(
            qr.starts_with("data:image/svg+xml;base64,"),
            "应是 SVG data URL"
        );
        // 解码回 SVG,确认是真实矢量二维码。
        let b64 = qr.trim_start_matches("data:image/svg+xml;base64,");
        let svg = String::from_utf8(b64_decode(b64)).unwrap();
        assert!(svg.contains("<svg"), "应含 <svg> 根节点");
    }

    /// CLI-written PNG → data URL; signature + IEND tail validated, non-PNG / truncated bytes rejected (caller falls back to make_qr).
    #[test]
    fn png_data_url_validates_signature() {
        let png = b"\x89PNG\r\n\x1a\nrest-of-file\x00\x00\x00\x00IEND\xae\x42\x60\x82";
        let data_url = png_data_url(png).expect("bytes with signature and IEND tail should pass");
        assert!(data_url.starts_with("data:image/png;base64,"));
        // Decode back to the original bytes to confirm losslessness.
        let b64 = data_url.trim_start_matches("data:image/png;base64,");
        assert_eq!(b64_decode(b64), png.to_vec());

        assert!(
            png_data_url(b"").is_none(),
            "empty bytes should be rejected"
        );
        assert!(
            png_data_url(b"\x89PNG").is_none(),
            "shorter than the signature should be rejected"
        );
        assert!(
            png_data_url(b"\x89PNG\r\n\x1a\n").is_none(),
            "signature without IEND tail (half-written file) should be rejected"
        );
        assert!(
            png_data_url(b"\x89PNG\r\n\x1a\nbody\x00\x00\x00\x00IEND\x00\x00\x00\x00").is_none(),
            "mismatched IEND CRC should be rejected"
        );
        assert!(
            png_data_url(b"\x89JPEG\r\n\x1a\nrest").is_none(),
            "wrong signature should be rejected"
        );
    }

    /// The timeout path must stay bounded and reap the killed child: a
    /// failed kill() returns immediately instead of waiting on a
    /// still-alive installer, and after a confirmed kill the reap is
    /// bounded by REAP_GRACE. Kill and reap are both pinned via the error
    /// text: a failed kill() and an abandoned or failed reap each append
    /// a clause, so their absence proves the installer was killed and
    /// collected. Soft-skips when no `sleep` binary is on PATH (bare
    /// Windows hosts; windows-latest CI bash steps have Git Bash's
    /// `sleep.exe` on PATH, so the test runs for real there). The
    /// Abandoned reap branch is induced deterministically in the
    /// platform::process tests with a zero grace budget; the Failed
    /// branch is not inducible in-process (std caches the exit status, so
    /// a collected child can never report a wait error).
    #[test]
    fn run_with_timeout_reaps_and_stays_bounded() {
        if Command::new("sleep").arg("0").status().is_err() {
            eprintln!("skipping: no `sleep` binary on this platform");
            return;
        }
        let _lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _env = crate::platform::paths::tests::EnvVarGuard::capture(&["PINVOU3_HOME"]);
        let root = std::env::temp_dir().join(format!(
            "pinvou3-rwt-reap-test-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        std::fs::create_dir_all(&root).expect("create temp PINVOU3_HOME");
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes
        // serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &root) };

        let started = Instant::now();
        let mut cmd = Command::new("sleep");
        cmd.arg("120");
        let error = run_with_timeout(cmd, 1).expect_err("sleep 120s must hit the 1s timeout");
        let _ = std::fs::remove_dir_all(&root);

        assert!(
            error.contains("CLI 安装超时(1s)"),
            "unexpected timeout error: {error}"
        );
        assert!(
            !error.contains("进程可能驻留")
                && !error.contains("回收安装进程失败")
                && !error.contains("终止安装进程失败"),
            "timeout error must not contain kill or reap failure clauses: {error}"
        );
        let elapsed = started.elapsed();
        // >= 1s proves the timeout poll loop actually ran (a spawn failure
        // would also Err but return instantly); < 10s proves the timeout
        // path returned promptly after kill + grace-bounded reap (normal
        // exit is milliseconds; the 2s REAP_GRACE is the worst-case tail)
        // instead of blocking on the child indefinitely.
        assert!(
            elapsed >= Duration::from_secs(1) && elapsed < Duration::from_secs(10),
            "timeout path should return promptly after the kill, took {elapsed:?}"
        );
    }

    // 测试辅助:标准 base64 解码(生产 b64 的逆运算,仅测试用)。
    fn b64_decode(s: &str) -> Vec<u8> {
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let val = |c: u8| A.iter().position(|&x| x == c).unwrap() as u32;
        let mut out = Vec::new();
        let clean: Vec<u8> = s.bytes().filter(|&c| c != b'=').collect();
        for chunk in clean.chunks(4) {
            let mut n = 0u32;
            for (i, &c) in chunk.iter().enumerate() {
                n |= val(c) << (18 - 6 * i);
            }
            out.push((n >> 16) as u8);
            if chunk.len() > 2 {
                out.push((n >> 8) as u8);
            }
            if chunk.len() > 3 {
                out.push(n as u8);
            }
        }
        out
    }
}
