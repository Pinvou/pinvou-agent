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
//! 连接状态按 connector id + generation 追踪每轮 lease、全部 owned process group 与取消标志；
//! `lib.rs` 里 `.manage(ConnectorConn::default())` 注册一次，四个连接器共用。

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter};

/// Resource Governor 可以治理的连接器全集。这里故意是编译期封闭集合；调用方不能
/// 传入连接器 id，更不能借 ConnectorConn 的通用槽位去控制未知进程。
const GOVERNED_CONNECTOR_IDS: [&str; 4] = ["feishu", "wecom", "dingtalk", "tmeet"];

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
        // 长驻 connector CLI 经常再派生 node / shell。Unix 上从创建时就把它放进
        // 独立进程组，后续取消才能按受信 root pid（同时也是 pgid）收掉整组；
        // Windows 由 taskkill /T 保持既有 tree-kill 语义。
        crate::platform::process::std_process_group_leader(&mut c);
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

/// 跑一个命令、收集 `(success, stdout, stderr)`。在 `spawn_blocking` 里调。
pub fn run(mut cmd: Command) -> Result<(bool, String, String), String> {
    let out = cmd
        .output()
        .map_err(|e| format!("启动失败: {e}(需要先完成对应连接器 CLI 的在线安装)"))?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

/// 在精确 connector lease 下运行一个会收集输出的 CLI 子进程。
///
/// 每个子进程从 spawn 起就是独立进程组，并在等待期间轮询 generation 取消标志。无论
/// 正常退出、超时还是取消，都会先确认整个进程组已经消失，再清理 PID；stop 结果未知时
/// ownership 会保留给 HostWork 后续 reconcile，绝不假报 Stopped。
pub fn run_owned(
    conn: &ConnectorConn,
    lease: ConnectorLease,
    mut cmd: Command,
    timeout: Duration,
) -> Result<Option<(bool, String, String)>, String> {
    use std::io::Read as _;

    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|error| format!("启动 connector CLI 失败: {error}"))?;
    let pid = child.id();
    let accepted = conn.register_pid(lease, pid).map_err(str::to_string)?;
    if !accepted {
        stop_registered_child(conn, lease, &mut child)?;
        return Ok(None);
    }

    let mut stdout = child
        .stdout
        .take()
        .ok_or("connector CLI stdout pipe is unavailable")?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or("connector CLI stderr pipe is unavailable")?;
    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        bytes
    });
    let started = Instant::now();

    let status = loop {
        if conn.is_cancelled(lease) {
            stop_registered_child(conn, lease, &mut child)?;
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Ok(None);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                // 父进程已经退出不代表其 descendants 已退出；按原 pgid 再做幂等 group stop。
                conn.stop_pid(lease, pid)?;
                break status;
            }
            Ok(None) if started.elapsed() <= timeout => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                stop_registered_child(conn, lease, &mut child)?;
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!("connector CLI 超时({}s)", timeout.as_secs()));
            }
            Err(error) => {
                stop_registered_child(conn, lease, &mut child)?;
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!("connector CLI 等待失败: {error}"));
            }
        }
    };

    Ok(Some((
        status.success(),
        String::from_utf8_lossy(&stdout_reader.join().unwrap_or_default()).into_owned(),
        String::from_utf8_lossy(&stderr_reader.join().unwrap_or_default()).into_owned(),
    )))
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
                    let _ = child.kill();
                    return Err(format!(
                        "CLI 安装超时({secs}s):可能是网络 / 代理(Clash)拦截(日志见 {})",
                        log_path.display()
                    ));
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    }
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

/// 把授权 URL 渲染成二维码(SVG data URL,供前端 `<img src>` 直接显示)。
/// 纯 Rust(qrcode crate),不依赖具体 CLI 的 qrcode 子命令——各连接器通用。
/// 失败返回 `None`(前端回退开浏览器)。
pub fn make_qr(url: &str) -> Option<String> {
    use qrcode::render::svg;
    use qrcode::QrCode;
    let code = QrCode::new(url.as_bytes()).ok()?;
    let svg_xml = code
        .render::<svg::Color>()
        .min_dimensions(220, 220)
        .quiet_zone(true)
        .build();
    Some(format!(
        "data:image/svg+xml;base64,{}",
        b64(svg_xml.as_bytes())
    ))
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

/// tree-kill 一个由 [`CliCtx::base_cmd`] 创建的 owned root PID。
///
/// Unix 上该 PID 同时是独立进程组 id；Windows 使用 `taskkill /T`。返回错误而不是
/// 假装已派发，HostWork worker 会把不确定结果留给后验 status reconcile。
pub fn kill_pid_tree(pid: u32) -> Result<(), String> {
    crate::platform::process::kill_process_tree(pid)
        .map_err(|error| format!("stop connector process group {pid}: {error}"))
}

/// 给前端发连接编排事件(`<id>:qr` / `<id>:phase` / `<id>:connected` / `<id>:error`)。
pub fn emit(app: &AppHandle, event: &str, payload: Value) {
    let _ = app.emit(event, payload);
}

// ──────────────────────── 多连接器共享的连接编排状态 ────────────────────────

/// 一次连接尝试的稳定所有权句柄。
///
/// generation 由宿主生成且只在进程内有效。旧任务只能清理自己的 generation，不能把
/// 后启动任务登记的 PID 擦掉；同一连接器并发启动时，每一轮都保留独立的受管进程组。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectorLease {
    id: &'static str,
    generation: u64,
}

/// 按连接器 id + generation 存所有活跃连接尝试。`lib.rs` 注册一次，四个连接器共用。
#[derive(Default)]
pub struct ConnectorConn {
    slots: Mutex<HashMap<&'static str, Slot>>,
}

#[derive(Default)]
struct Slot {
    next_generation: u64,
    runs: BTreeMap<u64, ConnectorRun>,
}

#[derive(Default)]
struct ConnectorRun {
    pid: Option<u32>,
    cancelled: bool,
}

impl ConnectorConn {
    /// 在任何后台工作启动前登记一轮连接尝试。
    pub fn begin(&self, id: &'static str) -> Result<ConnectorLease, &'static str> {
        let mut slots = self
            .slots
            .lock()
            .map_err(|_| "connector ownership registry is unavailable")?;
        let slot = slots.entry(id).or_default();
        let generation = slot
            .next_generation
            .checked_add(1)
            .ok_or("connector ownership generation is exhausted")?;
        slot.next_generation = generation;
        slot.runs.insert(generation, ConnectorRun::default());
        Ok(ConnectorLease { id, generation })
    }

    /// 置该连接器所有活跃 generation 的取消标志，并返回目前登记的全部 owned PID。
    pub fn cancel(&self, id: &'static str) -> Result<Vec<u32>, &'static str> {
        let mut slots = self
            .slots
            .lock()
            .map_err(|_| "connector ownership registry is unavailable")?;
        let Some(slot) = slots.get_mut(id) else {
            return Ok(Vec::new());
        };
        let mut pids = Vec::new();
        for run in slot.runs.values_mut() {
            run.cancelled = true;
            if let Some(pid) = run.pid {
                pids.push(pid);
            }
        }
        Ok(pids)
    }

    /// registry 不可用或 generation 已失效时 fail closed，调用方应停止刚启动的子进程。
    pub fn is_cancelled(&self, lease: ConnectorLease) -> bool {
        self.slots
            .lock()
            .ok()
            .and_then(|slots| {
                slots
                    .get(lease.id)
                    .and_then(|slot| slot.runs.get(&lease.generation))
                    .map(|run| run.cancelled)
            })
            .unwrap_or(true)
    }

    /// 把新生成的 owned process-group root 绑定到精确 generation。
    ///
    /// 如果该 generation 已取消、已结束，或仍登记着另一个 PID，则拒绝接管；调用方必须
    /// 立即终止刚启动的进程，避免出现 registry 看不见的后台工作。
    pub fn register_pid(&self, lease: ConnectorLease, pid: u32) -> Result<bool, &'static str> {
        let mut slots = self
            .slots
            .lock()
            .map_err(|_| "connector ownership registry is unavailable")?;
        let Some(run) = slots
            .get_mut(lease.id)
            .and_then(|slot| slot.runs.get_mut(&lease.generation))
        else {
            return Ok(false);
        };
        match run.pid {
            None => {
                run.pid = Some(pid);
                Ok(!run.cancelled)
            }
            Some(existing) => Ok(existing == pid && !run.cancelled),
        }
    }

    /// 只清理精确 generation 当前登记的同一个 PID。旧任务不能清掉新任务的 PID。
    pub fn clear_pid(&self, lease: ConnectorLease, pid: u32) -> Result<bool, &'static str> {
        let mut slots = self
            .slots
            .lock()
            .map_err(|_| "connector ownership registry is unavailable")?;
        let Some(run) = slots
            .get_mut(lease.id)
            .and_then(|slot| slot.runs.get_mut(&lease.generation))
        else {
            return Ok(false);
        };
        if run.pid == Some(pid) {
            run.pid = None;
            return Ok(true);
        }
        Ok(false)
    }

    /// 确认精确 PID 的整个进程组已停止后，才清理 ownership。
    pub fn stop_pid(&self, lease: ConnectorLease, pid: u32) -> Result<(), String> {
        kill_pid_tree(pid)?;
        self.clear_pid(lease, pid)
            .map_err(str::to_string)
            .map(|_| ())
    }

    /// 结束精确 generation。若仍登记 PID，先做最后一次 fail-safe group stop；失败时保留
    /// generation，让治理状态继续诚实地显示 Running/Unknown。
    pub fn finish(&self, lease: ConnectorLease) -> Result<(), String> {
        let pid = self
            .slots
            .lock()
            .map_err(|_| "connector ownership registry is unavailable".to_string())?
            .get(lease.id)
            .and_then(|slot| slot.runs.get(&lease.generation))
            .and_then(|run| run.pid);
        if let Some(pid) = pid {
            self.stop_pid(lease, pid)?;
        }
        let mut slots = self
            .slots
            .lock()
            .map_err(|_| "connector ownership registry is unavailable".to_string())?;
        if let Some(slot) = slots.get_mut(lease.id) {
            slot.runs.remove(&lease.generation);
            // 保留空 slot 的 monotonic generation，避免 stale copied lease 与未来新轮次别名。
        }
        Ok(())
    }

    /// 只读返回固定白名单连接器当前活跃连接尝试数。即使子进程尚未登记或两阶段之间
    /// 暂无 PID，reservation 仍算 Running，避免 HostWork 假报 Stopped。
    pub(crate) fn governed_running_count(&self) -> Option<usize> {
        self.slots.lock().ok().map(|slots| {
            GOVERNED_CONNECTOR_IDS
                .iter()
                .filter_map(|id| slots.get(*id))
                .map(|slot| slot.runs.len())
                .sum()
        })
    }

    /// 对固定白名单连接器置取消标志，并只交出这些槽位中由 Pinvou 自己登记的 PID。
    /// 实际 tree-kill 由独立 HostWork worker 在锁外执行，避免慢进程等待阻塞其他槽位。
    pub(crate) fn cancel_governed(&self) -> Result<Vec<u32>, &'static str> {
        let mut slots = self
            .slots
            .lock()
            .map_err(|_| "connector ownership registry is unavailable")?;
        let mut pids = Vec::new();
        for id in GOVERNED_CONNECTOR_IDS {
            let Some(slot) = slots.get_mut(id) else {
                continue;
            };
            for run in slot.runs.values_mut() {
                run.cancelled = true;
                if let Some(pid) = run.pid {
                    pids.push(pid);
                }
            }
        }
        Ok(pids)
    }

    /// 停止一批已标记 cancelled 的 owned process group。
    ///
    /// 每个确认成功的 generation 都立即退役，只有失败的 generation 保留给下一轮
    /// HostWork reconcile。不能在部分失败时继续保留已经停止的旧 PGID：该数字一旦被
    /// 内核复用，后续重试可能误伤不属于 Pinvou 的新进程组。
    pub(crate) fn stop_cancelled_pids(&self, pids: Vec<u32>) -> Result<(), String> {
        self.stop_cancelled_pids_with(pids, kill_pid_tree)
    }

    fn stop_cancelled_pids_with<F>(&self, mut pids: Vec<u32>, mut stop: F) -> Result<(), String>
    where
        F: FnMut(u32) -> Result<(), String>,
    {
        pids.sort_unstable();
        pids.dedup();

        // 调用方交出的 PID 可能在 blocking worker 真正执行前已经被另一条精确清理路径
        // 退役。只对当前 registry 仍明确持有且已标记 cancelled 的 generation 动手，并
        // 保留精确 generation 身份供成功后清理，避免仅凭可复用的整数 PID 改写新状态。
        let targets = {
            let slots = self
                .slots
                .lock()
                .map_err(|_| "connector ownership registry is unavailable".to_string())?;
            slots
                .iter()
                .flat_map(|(id, slot)| {
                    slot.runs.iter().filter_map(|(generation, run)| {
                        let pid = run.pid?;
                        (run.cancelled && pids.binary_search(&pid).is_ok()).then_some((
                            *id,
                            *generation,
                            pid,
                        ))
                    })
                })
                .collect::<Vec<_>>()
        };
        let mut target_pids = targets.iter().map(|(_, _, pid)| *pid).collect::<Vec<_>>();
        target_pids.sort_unstable();
        target_pids.dedup();

        let mut errors = Vec::new();
        let mut stopped_pids = Vec::new();
        for pid in target_pids {
            match stop(pid) {
                Ok(()) => stopped_pids.push(pid),
                Err(error) => errors.push(error),
            }
        }

        let mut slots = self
            .slots
            .lock()
            .map_err(|_| "connector ownership registry is unavailable".to_string())?;
        for (id, generation, pid) in targets {
            if stopped_pids.binary_search(&pid).is_err() {
                continue;
            }
            let Some(slot) = slots.get_mut(id) else {
                continue;
            };
            if slot
                .runs
                .get(&generation)
                .is_some_and(|run| run.cancelled && run.pid == Some(pid))
            {
                slot.runs.remove(&generation);
            }
        }
        if !errors.is_empty() {
            return Err(errors.join("; "));
        }
        Ok(())
    }
}

/// 停止并回收一个已经登记的 child。只有 group stop 确认后才会 clear PID。
pub fn stop_registered_child(
    conn: &ConnectorConn,
    lease: ConnectorLease,
    child: &mut std::process::Child,
) -> Result<(), String> {
    let pid = child.id();
    conn.stop_pid(lease, pid)?;
    let _ = child.wait();
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct ConnectorAuthGateRefresh {
    feishu_visible: bool,
    wecom_visible: bool,
    dingtalk_visible: bool,
    tmeet_visible: bool,
    elapsed_ms: u64,
}

/// 首屏提交后刷新飞书 / 企微 / 钉钉 / 腾讯会议鉴权门控。外部 CLI 在 blocking 线程池并行执行，
/// 不占 Tauri setup 主线程；各自只修改互不重叠的技能目录。
pub async fn refresh_connector_auth_gates() -> Result<ConnectorAuthGateRefresh, String> {
    let started = Instant::now();
    crate::platform::startup::mark("connector_auth_refresh:start");

    let feishu = tokio::task::spawn_blocking(|| {
        let show = crate::features::connectors::feishu::feishu_skills_should_show();
        crate::features::runtime_bundle::platform::Pinvou3Bundle::paths()
            .apply_feishu_skills(show)
            .map_err(|e| format!("刷新飞书技能门控失败: {e}"))?;
        Ok::<bool, String>(show)
    });
    let wecom = tokio::task::spawn_blocking(|| {
        let show = crate::features::connectors::wecom::wecom_skills_should_show();
        crate::features::runtime_bundle::platform::Pinvou3Bundle::paths()
            .apply_wecom_skills(show)
            .map_err(|e| format!("刷新企微技能门控失败: {e}"))?;
        Ok::<bool, String>(show)
    });
    let dingtalk = tokio::task::spawn_blocking(|| {
        let show = crate::features::connectors::dingtalk::dingtalk_skills_should_show();
        crate::features::runtime_bundle::platform::Pinvou3Bundle::paths()
            .apply_dingtalk_skills(show)
            .map_err(|e| format!("刷新钉钉技能门控失败: {e}"))?;
        Ok::<bool, String>(show)
    });
    let tmeet = tokio::task::spawn_blocking(|| {
        let show = crate::features::connectors::tmeet::tmeet_skills_should_show();
        crate::features::runtime_bundle::platform::Pinvou3Bundle::paths()
            .apply_tmeet_skills(show)
            .map_err(|e| format!("刷新腾讯会议技能门控失败: {e}"))?;
        Ok::<bool, String>(show)
    });

    let (feishu_result, wecom_result, dingtalk_result, tmeet_result) =
        tokio::join!(feishu, wecom, dingtalk, tmeet);
    let feishu_visible = feishu_result.map_err(|e| format!("飞书鉴权探测任务失败: {e}"))??;
    let wecom_visible = wecom_result.map_err(|e| format!("企微鉴权探测任务失败: {e}"))??;
    let dingtalk_visible = dingtalk_result.map_err(|e| format!("钉钉鉴权探测任务失败: {e}"))??;
    let tmeet_visible = tmeet_result.map_err(|e| format!("腾讯会议鉴权探测任务失败: {e}"))??;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    crate::platform::startup::mark_with_detail(
        "rust",
        "connector_auth_refresh:done",
        &format!(
            "elapsed_ms={elapsed_ms} feishu_visible={feishu_visible} wecom_visible={wecom_visible} dingtalk_visible={dingtalk_visible} tmeet_visible={tmeet_visible}"
        ),
    );
    Ok(ConnectorAuthGateRefresh {
        feishu_visible,
        wecom_visible,
        dingtalk_visible,
        tmeet_visible,
        elapsed_ms,
    })
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

    /// 命中白名单域、且在空白处截断(扫码 URL 常带 `&` 查询串,不能被切断)。
    #[test]
    fn extract_url_picks_auth_domain_url() {
        assert_eq!(
            TEST_CTX.extract_url("请打开 https://work.weixin.qq.com/x?a=1&b=2 扫码"),
            Some("https://work.weixin.qq.com/x?a=1&b=2".to_string())
        );
    }

    /// 非白名单域不算本连接器的 URL。
    #[test]
    fn extract_url_rejects_non_auth_domain() {
        assert_eq!(TEST_CTX.extract_url("https://example.com/foo"), None);
    }

    /// 没有 URL 时返回 None。
    #[test]
    fn extract_url_none_without_url() {
        assert_eq!(TEST_CTX.extract_url("纯文本,没有链接"), None);
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

    #[test]
    fn governed_connector_scope_tracks_every_generation_and_excludes_unknown_slots() {
        let state = ConnectorConn::default();
        let feishu_old = state.begin("feishu").expect("old feishu lease");
        let feishu_new = state.begin("feishu").expect("new feishu lease");
        let wecom = state.begin("wecom").expect("wecom lease");
        let untrusted = state.begin("untrusted").expect("untrusted lease");
        assert_eq!(state.register_pid(feishu_old, 11), Ok(true));
        assert_eq!(state.register_pid(feishu_new, 12), Ok(true));
        assert_eq!(state.register_pid(wecom, 13), Ok(true));
        assert_eq!(state.register_pid(untrusted, 99), Ok(true));

        assert_eq!(state.governed_running_count(), Some(3));
        let mut pids = state.cancel_governed().expect("governed cancellation");
        pids.sort_unstable();
        assert_eq!(pids, vec![11, 12, 13]);
        assert!(state.is_cancelled(feishu_old));
        assert!(state.is_cancelled(feishu_new));
        assert!(state.is_cancelled(wecom));
        assert!(!state.is_cancelled(untrusted));
    }

    #[test]
    fn old_generation_cleanup_cannot_clear_or_finish_a_new_generation() {
        let state = ConnectorConn::default();
        let old = state.begin("feishu").expect("old lease");
        let new = state.begin("feishu").expect("new lease");
        assert_eq!(state.register_pid(old, 21), Ok(true));
        assert_eq!(state.register_pid(new, 22), Ok(true));

        assert_eq!(state.clear_pid(old, 22), Ok(false));
        assert_eq!(state.clear_pid(old, 21), Ok(true));
        assert_eq!(state.finish(old), Ok(()));
        assert_eq!(state.governed_running_count(), Some(1));
        assert_eq!(state.cancel("feishu"), Ok(vec![22]));
        assert!(state.is_cancelled(new));

        assert_eq!(state.clear_pid(new, 22), Ok(true));
        assert_eq!(state.finish(new), Ok(()));
        let later = state.begin("feishu").expect("later lease");
        assert_ne!(later, old, "empty slots must retain a monotonic generation");
        assert_ne!(later, new, "future leases must not alias stale copies");
    }

    #[test]
    fn cancelled_batch_attempts_every_group_and_retires_each_confirmed_success() {
        let state = ConnectorConn::default();
        let first = state.begin("wecom").expect("first lease");
        let second = state.begin("wecom").expect("second lease");
        assert_eq!(state.register_pid(first, 41), Ok(true));
        assert_eq!(state.register_pid(second, 42), Ok(true));
        let pids = state.cancel("wecom").expect("cancel all generations");

        let mut attempted = Vec::new();
        let error = state
            .stop_cancelled_pids_with(pids.clone(), |pid| {
                attempted.push(pid);
                (pid != 41)
                    .then_some(())
                    .ok_or_else(|| "first group failed".to_string())
            })
            .expect_err("partial stop must retain only the failed ownership record");
        assert_eq!(attempted, vec![41, 42]);
        assert_eq!(error, "first group failed");
        assert_eq!(state.governed_running_count(), Some(1));
        assert_eq!(state.cancel("wecom"), Ok(vec![41]));

        let mut retried = Vec::new();
        state
            .stop_cancelled_pids_with(pids, |pid| {
                retried.push(pid);
                Ok(())
            })
            .expect("only the failed group should be retried");
        assert_eq!(retried, vec![41]);
        assert_eq!(state.governed_running_count(), Some(0));
    }

    #[test]
    fn cancelled_or_finished_generation_cannot_claim_a_late_pid() {
        let state = ConnectorConn::default();
        let cancelled = state.begin("dingtalk").expect("cancelled lease");
        assert_eq!(state.cancel("dingtalk"), Ok(Vec::new()));
        assert_eq!(state.register_pid(cancelled, 31), Ok(false));
        assert_eq!(state.cancel("dingtalk"), Ok(vec![31]));
        assert_eq!(state.clear_pid(cancelled, 31), Ok(true));

        let finished = state.begin("dingtalk").expect("finished lease");
        assert_eq!(state.finish(finished), Ok(()));
        assert_eq!(state.register_pid(finished, 32), Ok(false));
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
