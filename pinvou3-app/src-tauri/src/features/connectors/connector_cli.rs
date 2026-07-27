//! 通用 CLI 连接器管道 —— 抽自 `feishu.rs`,供飞书 / 企微等"官方 CLI 连接器"共享。
//!
//! 设计(开发方案 C):公共的"起子进程 / 抑黑窗 / 抓授权 URL / 出二维码 / 收发事件 /
//! 取消"逻辑收口在此;各连接器(`feishu.rs` / `wecom.rs`)只持有自己的 [`CliCtx`]
//! 薄声明 + 一段连接编排函数,调本模块的公共件。
//!
//! 连接状态(长驻子进程 PID + 取消标志)用一个 [`ConnectorConn`] 按连接器 id 复用,
//! `lib.rs` 里 `.manage(ConnectorConn::default())` 注册一次,飞书 / 企微共用。

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter};

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

/// 跑一个命令、收集 `(success, stdout, stderr)`。在 `spawn_blocking` 里调。
pub fn run(mut cmd: Command) -> Result<(bool, String, String), String> {
    let out = cmd
        .output()
        .map_err(|e| format!("启动失败: {e}(需要对应 CLI；Linux ARM64 会优先使用内置 CLI)"))?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
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
        .map_err(|e| format!("启动失败: {e}(需要 npm/Node 才能执行动态安装兜底)"))?;
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
        for line in BufReader::new(r).lines().flatten() {
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

/// 给前端发连接编排事件(`<id>:qr` / `<id>:phase` / `<id>:connected` / `<id>:error`)。
pub fn emit(app: &AppHandle, event: &str, payload: Value) {
    let _ = app.emit(event, payload);
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
